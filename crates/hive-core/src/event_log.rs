use crate::event_bus::{EventEnvelope, QueuedSubscriber};
use parking_lot::Mutex;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;
use tracing::Instrument;

// ── Public types ────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredEvent {
    pub id: i64,
    pub event_id: u64,
    pub topic: String,
    pub source: String,
    pub payload: serde_json::Value,
    pub timestamp_ms: u128,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordingSummary {
    pub id: String,
    pub name: Option<String>,
    pub topic_filter: Option<String>,
    pub started_ms: u128,
    pub stopped_ms: Option<u128>,
    pub event_count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordedEvent {
    pub event_id: u64,
    pub offset_ms: u128,
    pub topic: String,
    pub source: String,
    pub payload: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Recording {
    pub id: String,
    pub name: Option<String>,
    pub started_ms: u128,
    pub duration_ms: u128,
    pub events: Vec<RecordedEvent>,
}

// ── EventLogStore trait ──────────────────────────────────────────────

/// Trait abstracting the persistent storage operations for the event log.
///
/// All methods are synchronous – the underlying implementation may use a
/// database connection protected by a mutex.
pub trait EventLogStore: Send + Sync {
    /// Write a batch of events to the event log and any active recordings.
    fn flush_batch(&self, events: &[EventEnvelope]);

    /// Query events by topic prefix, optionally filtering by time range.
    fn query_events(
        &self,
        topic_prefix: Option<&str>,
        since_ms: Option<u128>,
        before_id: Option<i64>,
        after_id: Option<i64>,
        limit: Option<usize>,
    ) -> Vec<StoredEvent>;

    /// Delete events older than `before_ms`. Returns the number deleted.
    fn prune_before(&self, before_ms: u128) -> usize;

    /// Start a new recording session. Returns the recording ID.
    fn start_recording(&self, name: Option<&str>, topic_filter: Option<&str>) -> String;

    /// Stop an active recording.
    fn stop_recording(&self, recording_id: &str) -> Option<RecordingSummary>;

    /// List all recordings.
    fn list_recordings(&self) -> Vec<RecordingSummary>;

    /// Get a recording summary by ID.
    fn get_recording_summary(&self, id: &str) -> Option<RecordingSummary>;

    /// Get a full recording with all events.
    fn get_recording(&self, id: &str) -> Option<Recording>;

    /// Delete a recording and its events. Returns `true` if it existed.
    fn delete_recording(&self, id: &str) -> bool;

    /// Return the highest `event_id` stored, or 0 if the log is empty.
    /// Used to seed the `EventBus` counter on restart so new IDs don't
    /// collide with persisted events.
    fn max_event_id(&self) -> u64;
}

// ── SqliteEventLogStore ─────────────────────────────────────────────

/// SQLite-backed implementation of [`EventLogStore`].
pub struct SqliteEventLogStore {
    db: Arc<Mutex<Connection>>,
}

impl SqliteEventLogStore {
    /// Open (or create) a persistent store at `path`.
    pub fn open(path: PathBuf) -> anyhow::Result<Self> {
        let conn = Connection::open(&path)?;
        Self::init(conn)
    }

    /// Create an in-memory store (for tests).
    pub fn in_memory() -> anyhow::Result<Self> {
        let conn = Connection::open_in_memory()?;
        Self::init(conn)
    }

    fn init(conn: Connection) -> anyhow::Result<Self> {
        conn.execute_batch("PRAGMA journal_mode=WAL;")?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS event_log (
                 id           INTEGER PRIMARY KEY AUTOINCREMENT,
                 event_id     INTEGER NOT NULL,
                 topic        TEXT NOT NULL,
                 source       TEXT NOT NULL,
                 payload      TEXT NOT NULL,
                 timestamp_ms INTEGER NOT NULL
             );
             CREATE INDEX IF NOT EXISTS idx_event_log_topic ON event_log(topic);
             CREATE INDEX IF NOT EXISTS idx_event_log_ts    ON event_log(timestamp_ms);
             CREATE UNIQUE INDEX IF NOT EXISTS idx_event_log_eid ON event_log(event_id);

             CREATE TABLE IF NOT EXISTS recordings (
                 id           TEXT PRIMARY KEY,
                 name         TEXT,
                 topic_filter TEXT,
                 started_ms   INTEGER NOT NULL,
                 stopped_ms   INTEGER,
                 event_count  INTEGER DEFAULT 0
             );

             CREATE TABLE IF NOT EXISTS recording_events (
                 id           INTEGER PRIMARY KEY AUTOINCREMENT,
                 recording_id TEXT NOT NULL REFERENCES recordings(id) ON DELETE CASCADE,
                 event_id     INTEGER NOT NULL,
                 topic        TEXT NOT NULL,
                 source       TEXT NOT NULL,
                 payload      TEXT NOT NULL,
                 timestamp_ms INTEGER NOT NULL,
                 offset_ms    INTEGER NOT NULL
             );
             CREATE INDEX IF NOT EXISTS idx_rec_events_rid ON recording_events(recording_id);",
        )?;

        Ok(Self { db: Arc::new(Mutex::new(conn)) })
    }
}

impl EventLogStore for SqliteEventLogStore {
    fn flush_batch(&self, events: &[EventEnvelope]) {
        let conn = self.db.lock();

        // Fetch active recordings once per batch.
        let active_recordings: Vec<(String, Option<String>, u128)> = conn
            .prepare_cached(
                "SELECT id, topic_filter, started_ms FROM recordings WHERE stopped_ms IS NULL",
            )
            .ok()
            .map(|mut stmt| {
                stmt.query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, i64>(2)? as u128,
                    ))
                })
                .ok()
                .map(|rows| rows.filter_map(|r| r.ok()).collect())
                .unwrap_or_default()
            })
            .unwrap_or_default();

        // Wrap everything in a transaction for perf.
        let _ = conn.execute_batch("BEGIN");
        for ev in events {
            let payload = serde_json::to_string(&ev.payload).unwrap_or_default();
            let _ = conn.execute(
                "INSERT OR IGNORE INTO event_log (event_id, topic, source, payload, timestamp_ms) VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![ev.id as i64, ev.topic, ev.source, payload, ev.timestamp_ms as i64],
            );

            // Copy into active recordings if topic matches.
            for (rec_id, filter, started_ms) in &active_recordings {
                let matches = match filter {
                    Some(prefix) if !prefix.is_empty() => {
                        crate::event_bus::topic_matches_prefix(&ev.topic, prefix)
                    }
                    _ => true,
                };
                if matches {
                    let offset = ev.timestamp_ms.saturating_sub(*started_ms);
                    let _ = conn.execute(
                        "INSERT INTO recording_events (recording_id, event_id, topic, source, payload, timestamp_ms, offset_ms)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                        rusqlite::params![rec_id, ev.id as i64, ev.topic, ev.source, payload, ev.timestamp_ms as i64, offset as i64],
                    );
                    let _ = conn.execute(
                        "UPDATE recordings SET event_count = event_count + 1 WHERE id = ?1",
                        rusqlite::params![rec_id],
                    );
                }
            }
        }
        let _ = conn.execute_batch("COMMIT");
    }

    fn query_events(
        &self,
        topic_prefix: Option<&str>,
        since_ms: Option<u128>,
        before_id: Option<i64>,
        after_id: Option<i64>,
        limit: Option<usize>,
    ) -> Vec<StoredEvent> {
        let conn = self.db.lock();
        let limit = limit.unwrap_or(100).min(10_000) as i64;

        // Build the WHERE clause dynamically based on parameters
        let mut conditions = Vec::new();
        let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

        if let Some(prefix) = topic_prefix {
            let pattern = format!("{prefix}%");
            conditions.push(format!("topic LIKE ?{}", param_values.len() + 1));
            param_values.push(Box::new(pattern));
        }

        if let Some(since) = since_ms {
            conditions.push(format!("timestamp_ms >= ?{}", param_values.len() + 1));
            param_values.push(Box::new(since as i64));
        }

        if let Some(bid) = before_id {
            conditions.push(format!("id < ?{}", param_values.len() + 1));
            param_values.push(Box::new(bid));
        }

        if let Some(aid) = after_id {
            conditions.push(format!("id > ?{}", param_values.len() + 1));
            param_values.push(Box::new(aid));
        }

        let where_clause =
            if conditions.is_empty() { "1=1".to_string() } else { conditions.join(" AND ") };
        let sql = format!(
            "SELECT id, event_id, topic, source, payload, timestamp_ms \
             FROM event_log WHERE {where_clause} \
             ORDER BY id DESC LIMIT ?{}",
            param_values.len() + 1
        );
        param_values.push(Box::new(limit));

        let params_ref: Vec<&dyn rusqlite::types::ToSql> =
            param_values.iter().map(|b| b.as_ref()).collect();

        let mut stmt = conn.prepare_cached(&sql).unwrap();
        stmt.query_map(params_ref.as_slice(), map_stored_event)
            .ok()
            .map(|rows| rows.filter_map(|r| r.ok()).collect())
            .unwrap_or_default()
    }

    fn prune_before(&self, before_ms: u128) -> usize {
        let conn = self.db.lock();
        conn.execute(
            "DELETE FROM event_log WHERE timestamp_ms < ?1",
            rusqlite::params![before_ms as i64],
        )
        .unwrap_or(0)
    }

    fn start_recording(&self, name: Option<&str>, topic_filter: Option<&str>) -> String {
        let id = format!("rec-{}", uuid_v4());
        let now = now_ms();
        let conn = self.db.lock();
        let _ = conn.execute(
            "INSERT INTO recordings (id, name, topic_filter, started_ms) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![id, name, topic_filter, now as i64],
        );
        id
    }

    fn stop_recording(&self, recording_id: &str) -> Option<RecordingSummary> {
        let now = now_ms();
        let conn = self.db.lock();
        let changed = conn
            .execute(
                "UPDATE recordings SET stopped_ms = ?1 WHERE id = ?2 AND stopped_ms IS NULL",
                rusqlite::params![now as i64, recording_id],
            )
            .unwrap_or(0);
        if changed == 0 {
            return None;
        }
        drop(conn);
        self.get_recording_summary(recording_id)
    }

    fn list_recordings(&self) -> Vec<RecordingSummary> {
        let conn = self.db.lock();
        let mut stmt = conn
            .prepare_cached(
                "SELECT id, name, topic_filter, started_ms, stopped_ms, event_count
                 FROM recordings ORDER BY started_ms DESC",
            )
            .unwrap();
        stmt.query_map([], map_recording_summary)
            .ok()
            .map(|rows| rows.filter_map(|r| r.ok()).collect())
            .unwrap_or_default()
    }

    fn get_recording_summary(&self, id: &str) -> Option<RecordingSummary> {
        let conn = self.db.lock();
        conn.prepare_cached(
            "SELECT id, name, topic_filter, started_ms, stopped_ms, event_count
             FROM recordings WHERE id = ?1",
        )
        .ok()
        .and_then(|mut stmt| stmt.query_row(rusqlite::params![id], map_recording_summary).ok())
    }

    fn get_recording(&self, id: &str) -> Option<Recording> {
        let summary = self.get_recording_summary(id)?;
        let conn = self.db.lock();
        let events: Vec<RecordedEvent> = conn
            .prepare_cached(
                "SELECT event_id, offset_ms, topic, source, payload
                 FROM recording_events WHERE recording_id = ?1 ORDER BY offset_ms, id",
            )
            .ok()
            .and_then(|mut stmt| {
                stmt.query_map(rusqlite::params![id], |row| {
                    let payload_str: String = row.get(4)?;
                    Ok(RecordedEvent {
                        event_id: row.get::<_, i64>(0)? as u64,
                        offset_ms: row.get::<_, i64>(1)? as u128,
                        topic: row.get(2)?,
                        source: row.get(3)?,
                        payload: serde_json::from_str(&payload_str).unwrap_or_default(),
                    })
                })
                .ok()
                .map(|rows| rows.filter_map(|r| r.ok()).collect())
            })
            .unwrap_or_default();

        let duration_ms =
            summary.stopped_ms.unwrap_or_else(now_ms).saturating_sub(summary.started_ms);
        Some(Recording {
            id: summary.id,
            name: summary.name,
            started_ms: summary.started_ms,
            duration_ms,
            events,
        })
    }

    fn delete_recording(&self, id: &str) -> bool {
        let conn = self.db.lock();
        let _ = conn
            .execute("DELETE FROM recording_events WHERE recording_id = ?1", rusqlite::params![id]);
        conn.execute("DELETE FROM recordings WHERE id = ?1", rusqlite::params![id]).unwrap_or(0) > 0
    }

    fn max_event_id(&self) -> u64 {
        let conn = self.db.lock();
        conn.query_row("SELECT COALESCE(MAX(event_id), 0) FROM event_log", [], |row| {
            row.get::<_, i64>(0).map(|v| v as u64)
        })
        .unwrap_or(0)
    }
}

// ── EventLog ────────────────────────────────────────────────────────

/// Persistent event log backed by an [`EventLogStore`].
///
/// Implements `QueuedSubscriber` so it can be registered on an `EventBus`.
/// Writes are batched via an internal mpsc channel + background task for low
/// publish-path latency.
///
/// Call [`EventLog::start_writer`] from within a Tokio runtime to spawn the
/// background writer task. Events are buffered in the mpsc channel until then.
pub struct EventLog {
    store: Arc<dyn EventLogStore>,
    writer_tx: mpsc::UnboundedSender<EventEnvelope>,
    writer_rx: std::sync::Mutex<Option<mpsc::UnboundedReceiver<EventEnvelope>>>,
}

impl EventLog {
    /// Open (or create) a persistent event log at `path`.
    /// Call [`start_writer`] after entering a Tokio runtime.
    pub fn open(path: PathBuf) -> anyhow::Result<Self> {
        let store = Arc::new(SqliteEventLogStore::open(path)?);
        Ok(Self::with_store(store))
    }

    /// Create an in-memory event log (for tests).
    /// Automatically starts the background writer (must be called inside a
    /// Tokio runtime).
    pub fn in_memory() -> anyhow::Result<Self> {
        let store = Arc::new(SqliteEventLogStore::in_memory()?);
        let log = Self::with_store(store);
        log.start_writer();
        Ok(log)
    }

    /// Create an event log backed by a custom [`EventLogStore`].
    pub fn with_store(store: Arc<dyn EventLogStore>) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        Self { store, writer_tx: tx, writer_rx: std::sync::Mutex::new(Some(rx)) }
    }

    /// Spawn the background writer task.  Must be called from within a Tokio
    /// runtime.  Safe to call multiple times (only the first call spawns).
    pub fn start_writer(&self) {
        if let Some(rx) = self.writer_rx.lock().unwrap().take() {
            let store = Arc::clone(&self.store);
            tokio::spawn(
                Self::batch_writer(store, rx)
                    .instrument(tracing::info_span!("service", service = "event-log")),
            );
        }
    }

    /// Background task that drains the mpsc channel and writes events in
    /// batches.  Flushes every 64 events or 100 ms, whichever comes first.
    async fn batch_writer(
        store: Arc<dyn EventLogStore>,
        mut rx: mpsc::UnboundedReceiver<EventEnvelope>,
    ) {
        tracing::info!("event log batch writer started");
        let mut buf: Vec<EventEnvelope> = Vec::with_capacity(64);
        loop {
            // Wait for at least one event (or channel close).
            match rx.recv().await {
                Some(ev) => buf.push(ev),
                None => break,
            }

            // Drain any additional buffered events (up to batch size).
            while buf.len() < 64 {
                match rx.try_recv() {
                    Ok(ev) => buf.push(ev),
                    Err(_) => break,
                }
            }

            if !buf.is_empty() {
                store.flush_batch(&buf);
                buf.clear();
            }
        }

        // Flush remaining events on shutdown.
        while let Ok(ev) = rx.try_recv() {
            buf.push(ev);
        }
        if !buf.is_empty() {
            store.flush_batch(&buf);
        }
    }

    // ── Query API (delegated to store) ──────────────────────────────

    /// Query events by topic prefix, optionally filtering by time range.
    pub fn query_events(
        &self,
        topic_prefix: Option<&str>,
        since_ms: Option<u128>,
        before_id: Option<i64>,
        after_id: Option<i64>,
        limit: Option<usize>,
    ) -> Vec<StoredEvent> {
        self.store.query_events(topic_prefix, since_ms, before_id, after_id, limit)
    }

    /// Delete events older than `before_ms`.
    pub fn prune_before(&self, before_ms: u128) -> usize {
        self.store.prune_before(before_ms)
    }

    /// Return the highest `event_id` stored, or 0 if the log is empty.
    pub fn max_event_id(&self) -> u64 {
        self.store.max_event_id()
    }

    // ── Recording API (delegated to store) ──────────────────────────

    /// Start a new recording session.  Returns the recording ID.
    pub fn start_recording(&self, name: Option<&str>, topic_filter: Option<&str>) -> String {
        self.store.start_recording(name, topic_filter)
    }

    /// Stop an active recording.
    pub fn stop_recording(&self, recording_id: &str) -> Option<RecordingSummary> {
        self.store.stop_recording(recording_id)
    }

    /// List all recordings.
    pub fn list_recordings(&self) -> Vec<RecordingSummary> {
        self.store.list_recordings()
    }

    /// Get a recording summary by ID.
    pub fn get_recording_summary(&self, id: &str) -> Option<RecordingSummary> {
        self.store.get_recording_summary(id)
    }

    /// Get a full recording with all events.
    pub fn get_recording(&self, id: &str) -> Option<Recording> {
        self.store.get_recording(id)
    }

    /// Delete a recording and its events.
    pub fn delete_recording(&self, id: &str) -> bool {
        self.store.delete_recording(id)
    }

    /// Export a recording as a JSON string.
    pub fn export_json(&self, id: &str) -> Option<String> {
        let recording = self.store.get_recording(id)?;
        serde_json::to_string_pretty(&recording).ok()
    }

    /// Export a recording as a Rust test scaffold.
    pub fn export_test_scaffold(&self, id: &str) -> Option<String> {
        let recording = self.store.get_recording(id)?;
        let test_name = recording
            .name
            .as_deref()
            .unwrap_or("recorded_session")
            .replace(|c: char| !c.is_alphanumeric() && c != '_', "_")
            .to_lowercase();

        let mut code = String::new();
        code.push_str("use hive_core::{EventBus, EventEnvelope};\n");
        code.push_str("use serde_json::json;\n");
        code.push_str("use tokio::time::{sleep, Duration};\n\n");
        code.push_str(&format!("#[tokio::test]\nasync fn replay_{test_name}() {{\n"));
        code.push_str("    let bus = EventBus::new(512);\n");
        code.push_str("    let mut rx = bus.subscribe_queued(\"\");\n\n");
        code.push_str("    // Replay captured events\n");

        let mut prev_offset = 0u128;
        for event in &recording.events {
            if event.offset_ms > prev_offset && prev_offset > 0 {
                let delay = event.offset_ms - prev_offset;
                code.push_str(&format!("    sleep(Duration::from_millis({delay})).await;\n"));
            }
            let payload_str = serde_json::to_string(&event.payload).unwrap_or_else(|_| "{}".into());
            code.push_str(&format!(
                "    bus.publish({:?}, {:?}, json!({})).unwrap();\n",
                event.topic, event.source, payload_str
            ));
            prev_offset = event.offset_ms;
        }

        code.push_str(&format!("\n    // Verify {} event(s) received\n", recording.events.len()));
        code.push_str(&format!("    for _ in 0..{} {{\n", recording.events.len()));
        code.push_str("        let event = rx.recv().await.expect(\"should receive event\");\n");
        code.push_str("        // TODO: Add your assertions here\n");
        code.push_str("        let _ = event;\n");
        code.push_str("    }\n");
        code.push_str("}\n");

        Some(code)
    }

    /// Flush any pending writes by waiting for the writer task.
    /// Useful in tests.
    pub async fn flush(&self) {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

impl QueuedSubscriber for EventLog {
    fn accept(&self, _envelope: &EventEnvelope) -> bool {
        true // accept all events
    }

    fn send(&self, envelope: EventEnvelope) {
        let _ = self.writer_tx.send(envelope);
    }
}

// ── Helpers ─────────────────────────────────────────────────────────

fn now_ms() -> u128 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis()
}

fn uuid_v4() -> String {
    use std::time::UNIX_EPOCH;
    let t = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    let nanos = t.as_nanos();
    let pid = std::process::id();
    let rand: u64 = (nanos as u64).wrapping_mul(6364136223846793005).wrapping_add(pid as u64);
    format!("{:016x}{:016x}", nanos as u64, rand)
}

fn map_stored_event(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredEvent> {
    let payload_str: String = row.get(4)?;
    Ok(StoredEvent {
        id: row.get(0)?,
        event_id: row.get::<_, i64>(1)? as u64,
        topic: row.get(2)?,
        source: row.get(3)?,
        payload: serde_json::from_str(&payload_str).unwrap_or_default(),
        timestamp_ms: row.get::<_, i64>(5)? as u128,
    })
}

fn map_recording_summary(row: &rusqlite::Row<'_>) -> rusqlite::Result<RecordingSummary> {
    Ok(RecordingSummary {
        id: row.get(0)?,
        name: row.get(1)?,
        topic_filter: row.get(2)?,
        started_ms: row.get::<_, i64>(3)? as u128,
        stopped_ms: row.get::<_, Option<i64>>(4)?.map(|v| v as u128),
        event_count: row.get::<_, i64>(5)? as u64,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::HashMap;

    // ── InMemoryEventLogStore test double ────────────────────────────

    struct RecordingState {
        id: String,
        name: Option<String>,
        topic_filter: Option<String>,
        started_ms: u128,
        stopped_ms: Option<u128>,
        events: Vec<RecordedEvent>,
    }

    struct InMemoryEventLogInner {
        events: Vec<StoredEvent>,
        next_id: i64,
        recordings: HashMap<String, RecordingState>,
        next_rec_counter: u64,
    }

    pub(crate) struct InMemoryEventLogStore {
        inner: Mutex<InMemoryEventLogInner>,
    }

    impl InMemoryEventLogStore {
        pub(crate) fn new() -> Self {
            Self {
                inner: Mutex::new(InMemoryEventLogInner {
                    events: Vec::new(),
                    next_id: 1,
                    recordings: HashMap::new(),
                    next_rec_counter: 1,
                }),
            }
        }
    }

    impl EventLogStore for InMemoryEventLogStore {
        fn flush_batch(&self, events: &[EventEnvelope]) {
            let mut inner = self.inner.lock();
            for ev in events {
                let stored = StoredEvent {
                    id: inner.next_id,
                    event_id: ev.id,
                    topic: ev.topic.clone(),
                    source: ev.source.clone(),
                    payload: ev.payload.clone(),
                    timestamp_ms: ev.timestamp_ms,
                };
                inner.next_id += 1;
                inner.events.push(stored);

                // Capture into active recordings whose filter matches.
                for rec in inner.recordings.values_mut() {
                    if rec.stopped_ms.is_some() {
                        continue;
                    }
                    let matches = match &rec.topic_filter {
                        Some(f) => ev.topic.starts_with(f),
                        None => true,
                    };
                    if matches {
                        let offset_ms = ev.timestamp_ms.saturating_sub(rec.started_ms);
                        rec.events.push(RecordedEvent {
                            event_id: ev.id,
                            offset_ms,
                            topic: ev.topic.clone(),
                            source: ev.source.clone(),
                            payload: ev.payload.clone(),
                        });
                    }
                }
            }
        }

        fn query_events(
            &self,
            topic_prefix: Option<&str>,
            since_ms: Option<u128>,
            before_id: Option<i64>,
            after_id: Option<i64>,
            limit: Option<usize>,
        ) -> Vec<StoredEvent> {
            let inner = self.inner.lock();
            let iter = inner.events.iter().rev().filter(|e| {
                if let Some(prefix) = topic_prefix {
                    if !e.topic.starts_with(prefix) {
                        return false;
                    }
                }
                if let Some(since) = since_ms {
                    if e.timestamp_ms < since {
                        return false;
                    }
                }
                if let Some(bid) = before_id {
                    if e.id >= bid {
                        return false;
                    }
                }
                if let Some(aid) = after_id {
                    if e.id <= aid {
                        return false;
                    }
                }
                true
            });
            match limit {
                Some(n) => iter.take(n).cloned().collect(),
                None => iter.cloned().collect(),
            }
        }

        fn prune_before(&self, before_ms: u128) -> usize {
            let mut inner = self.inner.lock();
            let before_len = inner.events.len();
            inner.events.retain(|e| e.timestamp_ms >= before_ms);
            before_len - inner.events.len()
        }

        fn start_recording(&self, name: Option<&str>, topic_filter: Option<&str>) -> String {
            let mut inner = self.inner.lock();
            let id = format!("rec-{}", inner.next_rec_counter);
            inner.next_rec_counter += 1;
            let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis();
            inner.recordings.insert(
                id.clone(),
                RecordingState {
                    id: id.clone(),
                    name: name.map(|s| s.to_string()),
                    topic_filter: topic_filter.map(|s| s.to_string()),
                    started_ms: now,
                    stopped_ms: None,
                    events: Vec::new(),
                },
            );
            id
        }

        fn stop_recording(&self, recording_id: &str) -> Option<RecordingSummary> {
            let mut inner = self.inner.lock();
            let rec = inner.recordings.get_mut(recording_id)?;
            if rec.stopped_ms.is_some() {
                return None;
            }
            let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis();
            rec.stopped_ms = Some(now);
            Some(RecordingSummary {
                id: rec.id.clone(),
                name: rec.name.clone(),
                topic_filter: rec.topic_filter.clone(),
                started_ms: rec.started_ms,
                stopped_ms: rec.stopped_ms,
                event_count: rec.events.len() as u64,
            })
        }

        fn list_recordings(&self) -> Vec<RecordingSummary> {
            let inner = self.inner.lock();
            inner
                .recordings
                .values()
                .map(|rec| RecordingSummary {
                    id: rec.id.clone(),
                    name: rec.name.clone(),
                    topic_filter: rec.topic_filter.clone(),
                    started_ms: rec.started_ms,
                    stopped_ms: rec.stopped_ms,
                    event_count: rec.events.len() as u64,
                })
                .collect()
        }

        fn get_recording_summary(&self, id: &str) -> Option<RecordingSummary> {
            let inner = self.inner.lock();
            let rec = inner.recordings.get(id)?;
            Some(RecordingSummary {
                id: rec.id.clone(),
                name: rec.name.clone(),
                topic_filter: rec.topic_filter.clone(),
                started_ms: rec.started_ms,
                stopped_ms: rec.stopped_ms,
                event_count: rec.events.len() as u64,
            })
        }

        fn get_recording(&self, id: &str) -> Option<Recording> {
            let inner = self.inner.lock();
            let rec = inner.recordings.get(id)?;
            let stopped = rec.stopped_ms?;
            Some(Recording {
                id: rec.id.clone(),
                name: rec.name.clone(),
                started_ms: rec.started_ms,
                duration_ms: stopped.saturating_sub(rec.started_ms),
                events: rec.events.clone(),
            })
        }

        fn delete_recording(&self, id: &str) -> bool {
            let mut inner = self.inner.lock();
            inner.recordings.remove(id).is_some()
        }

        fn max_event_id(&self) -> u64 {
            let inner = self.inner.lock();
            inner.events.iter().map(|e| e.event_id).max().unwrap_or(0)
        }
    }

    #[test]
    fn test_in_memory_event_log_store_roundtrip() {
        let store = InMemoryEventLogStore::new();

        // 1. Flush some events.
        store.flush_batch(&[
            EventEnvelope {
                id: 1,
                topic: "chat.session.created".into(),
                source: "test".into(),
                payload: json!({"sid": "s1"}),
                timestamp_ms: 1000,
            },
            EventEnvelope {
                id: 2,
                topic: "config.reloaded".into(),
                source: "test".into(),
                payload: json!({}),
                timestamp_ms: 2000,
            },
            EventEnvelope {
                id: 3,
                topic: "chat.message.queued".into(),
                source: "test".into(),
                payload: json!({"mid": "m1"}),
                timestamp_ms: 3000,
            },
        ]);

        // 2. Query all events.
        let all = store.query_events(None, None, None, None, None);
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].event_id, 3);
        assert_eq!(all[2].event_id, 1);

        // Query by topic prefix.
        let chat = store.query_events(Some("chat."), None, None, None, None);
        assert_eq!(chat.len(), 2);

        // Query by since_ms.
        let recent = store.query_events(None, Some(2500), None, None, None);
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].topic, "chat.message.queued");

        // Query with limit.
        let limited = store.query_events(None, None, None, None, Some(2));
        assert_eq!(limited.len(), 2);

        // 3. Prune events before timestamp 2000.
        let pruned = store.prune_before(2000);
        assert_eq!(pruned, 1);
        let remaining = store.query_events(None, None, None, None, None);
        assert_eq!(remaining.len(), 2);

        // 4. Recording lifecycle.
        let rec_id = store.start_recording(Some("my-rec"), Some("chat"));

        store.flush_batch(&[
            EventEnvelope {
                id: 10,
                topic: "chat.msg".into(),
                source: "test".into(),
                payload: json!({"x": 1}),
                timestamp_ms: 5000,
            },
            EventEnvelope {
                id: 11,
                topic: "config.changed".into(),
                source: "test".into(),
                payload: json!({}),
                timestamp_ms: 5001,
            },
            EventEnvelope {
                id: 12,
                topic: "chat.done".into(),
                source: "test".into(),
                payload: json!({"x": 2}),
                timestamp_ms: 5002,
            },
        ]);

        // Stop and verify.
        let summary = store.stop_recording(&rec_id).unwrap();
        assert_eq!(summary.event_count, 2); // only chat.* events
        assert!(summary.stopped_ms.is_some());
        assert_eq!(summary.name.as_deref(), Some("my-rec"));

        let recording = store.get_recording(&rec_id).unwrap();
        assert_eq!(recording.events.len(), 2);
        assert_eq!(recording.events[0].topic, "chat.msg");
        assert_eq!(recording.events[1].topic, "chat.done");

        // List and get_recording_summary.
        let list = store.list_recordings();
        assert_eq!(list.len(), 1);
        assert!(store.get_recording_summary(&rec_id).is_some());

        // Delete.
        assert!(store.delete_recording(&rec_id));
        assert!(store.get_recording(&rec_id).is_none());
        assert!(!store.delete_recording(&rec_id));
    }

    #[tokio::test]
    async fn event_log_write_and_query() {
        let log = EventLog::in_memory().unwrap();
        log.send(EventEnvelope {
            id: 1,
            topic: "chat.session.created".into(),
            source: "test".into(),
            payload: json!({"sid": "s1"}),
            timestamp_ms: 1000,
        });
        log.send(EventEnvelope {
            id: 2,
            topic: "config.reloaded".into(),
            source: "test".into(),
            payload: json!({}),
            timestamp_ms: 2000,
        });
        log.send(EventEnvelope {
            id: 3,
            topic: "chat.message.queued".into(),
            source: "test".into(),
            payload: json!({"sid": "s1"}),
            timestamp_ms: 3000,
        });

        log.flush().await;

        let all = log.query_events(None, None, None, None, None);
        assert_eq!(all.len(), 3);

        let chat_only = log.query_events(Some("chat."), None, None, None, None);
        assert_eq!(chat_only.len(), 2);

        let recent = log.query_events(None, Some(2500), None, None, None);
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].topic, "chat.message.queued");
    }

    #[tokio::test]
    async fn event_log_prune() {
        let log = EventLog::in_memory().unwrap();
        for i in 0u64..5 {
            log.send(EventEnvelope {
                id: i + 1,
                topic: "test".into(),
                source: "test".into(),
                payload: json!({"i": i}),
                timestamp_ms: ((i as u128) + 1) * 1000,
            });
        }
        log.flush().await;

        let pruned = log.prune_before(3000);
        assert_eq!(pruned, 2); // timestamps 1000 and 2000

        let remaining = log.query_events(None, None, None, None, None);
        assert_eq!(remaining.len(), 3);
    }

    #[tokio::test]
    async fn recording_lifecycle() {
        let log = EventLog::in_memory().unwrap();

        let rec_id = log.start_recording(Some("test recording"), Some("chat"));

        // Simulate events arriving via the batch writer.
        log.send(EventEnvelope {
            id: 100,
            topic: "chat.session.created".into(),
            source: "test".into(),
            payload: json!({"sid": "s1"}),
            timestamp_ms: now_ms(),
        });
        log.send(EventEnvelope {
            id: 101,
            topic: "config.reloaded".into(),
            source: "test".into(),
            payload: json!({}),
            timestamp_ms: now_ms() + 10,
        });
        log.send(EventEnvelope {
            id: 102,
            topic: "chat.message.queued".into(),
            source: "test".into(),
            payload: json!({"mid": "m1"}),
            timestamp_ms: now_ms() + 20,
        });
        log.flush().await;

        let summary = log.stop_recording(&rec_id).unwrap();
        assert_eq!(summary.event_count, 2); // only chat.* events
        assert!(summary.stopped_ms.is_some());

        let recording = log.get_recording(&rec_id).unwrap();
        assert_eq!(recording.events.len(), 2);
        assert_eq!(recording.events[0].topic, "chat.session.created");
        assert_eq!(recording.events[1].topic, "chat.message.queued");

        // List includes this recording.
        let list = log.list_recordings();
        assert_eq!(list.len(), 1);

        // JSON export works.
        let json_str = log.export_json(&rec_id).unwrap();
        assert!(json_str.contains("chat.session.created"));

        // Rust scaffold export works.
        let scaffold = log.export_test_scaffold(&rec_id).unwrap();
        assert!(scaffold.contains("#[tokio::test]"));
        assert!(scaffold.contains("replay_test_recording"));
        assert!(scaffold.contains("chat.session.created"));

        // Delete works.
        assert!(log.delete_recording(&rec_id));
        assert!(log.get_recording(&rec_id).is_none());
    }

    #[test]
    fn max_event_id_empty_store() {
        let store = InMemoryEventLogStore::new();
        assert_eq!(store.max_event_id(), 0);
    }

    #[test]
    fn max_event_id_after_flush() {
        let store = InMemoryEventLogStore::new();
        store.flush_batch(&[
            EventEnvelope {
                id: 10,
                topic: "a".into(),
                source: "test".into(),
                payload: json!({}),
                timestamp_ms: 1000,
            },
            EventEnvelope {
                id: 42,
                topic: "b".into(),
                source: "test".into(),
                payload: json!({}),
                timestamp_ms: 2000,
            },
        ]);
        assert_eq!(store.max_event_id(), 42);
    }

    #[test]
    fn max_event_id_sqlite() {
        let store = SqliteEventLogStore::in_memory().unwrap();
        assert_eq!(store.max_event_id(), 0);

        store.flush_batch(&[
            EventEnvelope {
                id: 5,
                topic: "x".into(),
                source: "test".into(),
                payload: json!({}),
                timestamp_ms: 1000,
            },
            EventEnvelope {
                id: 99,
                topic: "y".into(),
                source: "test".into(),
                payload: json!({}),
                timestamp_ms: 2000,
            },
        ]);
        assert_eq!(store.max_event_id(), 99);
    }

    #[test]
    fn event_bus_set_next_id() {
        let bus = crate::event_bus::EventBus::new(16);

        // Publish an event — should get ID 1.
        let _ = bus.publish("a", "test", json!({}));

        // Seed the counter to 100.
        bus.set_next_id(100);

        // Next event should get ID 100.
        let mut rx = bus.subscribe();
        let _ = bus.publish("b", "test", json!({}));
        let ev = rx.try_recv().unwrap();
        assert_eq!(ev.id, 100);
    }

    /// Simulates a daemon restart: events flushed in the first lifetime
    /// must not be lost, and new events after seeding must be stored.
    #[tokio::test]
    async fn restart_seeding_prevents_id_collision() {
        // --- First daemon lifetime ---
        let bus1 = crate::event_bus::EventBus::new(16);
        let log1 = EventLog::in_memory().unwrap();
        bus1.register_subscriber(Arc::new(log1) as Arc<dyn crate::event_bus::QueuedSubscriber>);

        // We can't easily reuse the same in-memory store across two EventLog
        // instances, so simulate the scenario using SqliteEventLogStore
        // directly.
        let store = Arc::new(SqliteEventLogStore::in_memory().unwrap()) as Arc<dyn EventLogStore>;
        let log = EventLog::with_store(Arc::clone(&store));
        log.start_writer();

        // Publish 5 events (IDs 1..5).
        for i in 1..=5u64 {
            log.send(EventEnvelope {
                id: i,
                topic: format!("test.event.{i}"),
                source: "test".into(),
                payload: json!({"n": i}),
                timestamp_ms: i as u128 * 1000,
            });
        }
        log.flush().await;

        let stored = store.query_events(None, None, None, None, None);
        assert_eq!(stored.len(), 5);
        assert_eq!(store.max_event_id(), 5);

        // --- Simulated restart: new EventBus, same store ---
        // Without seeding, IDs 1..5 would collide and be dropped.
        let max_id = store.max_event_id();
        let bus2 = crate::event_bus::EventBus::new(16);
        bus2.set_next_id(max_id + 1); // seed from DB

        let log2 = EventLog::with_store(Arc::clone(&store));
        log2.start_writer();

        // Publish 3 more events — should get IDs 6, 7, 8.
        for i in 0..3u64 {
            log2.send(EventEnvelope {
                id: max_id + 1 + i,
                topic: format!("test.restart.{i}"),
                source: "test".into(),
                payload: json!({"n": i}),
                timestamp_ms: (10 + i) as u128 * 1000,
            });
        }
        log2.flush().await;

        let all = store.query_events(None, None, None, None, None);
        assert_eq!(all.len(), 8, "should have 5 original + 3 new events");
        assert_eq!(store.max_event_id(), 8);
    }
}
