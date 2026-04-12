use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::field::{Field, Visit};
use tracing::span;
use tracing_subscriber::layer::Context;
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::Layer;

/// Default per-service ring buffer capacity.
const DEFAULT_BUFFER_CAPACITY: usize = 1000;

// ── Public types ────────────────────────────────────────────────────

/// A single captured log entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogEntry {
    pub timestamp_ms: u64,
    pub level: String,
    pub message: String,
    pub fields: HashMap<String, String>,
    pub target: String,
}

/// Query parameters for filtering log entries.
#[derive(Debug, Default)]
pub struct LogQuery {
    pub since_ms: Option<u64>,
    pub limit: Option<usize>,
    pub level: Option<String>,
    pub search: Option<String>,
}

// ── Span extension data ─────────────────────────────────────────────

/// Data attached to spans so child events know which service they belong to.
#[derive(Debug, Clone)]
struct ServiceId(String);

// ── Collector ───────────────────────────────────────────────────────

/// In-memory per-service log ring buffer.
///
/// Shared via `Arc` between the tracing layer (writes) and the API layer
/// (reads). Each service id maps to a bounded `VecDeque<LogEntry>`.
#[derive(Debug, Clone)]
pub struct ServiceLogCollector {
    buffers: Arc<Mutex<HashMap<String, VecDeque<LogEntry>>>>,
    capacity: usize,
}

impl ServiceLogCollector {
    pub fn new() -> Self {
        Self { buffers: Arc::new(Mutex::new(HashMap::new())), capacity: DEFAULT_BUFFER_CAPACITY }
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self { buffers: Arc::new(Mutex::new(HashMap::new())), capacity: capacity.max(1) }
    }

    /// Return entries for the given service, filtered by `query`.
    pub fn get_logs(&self, service_id: &str, query: &LogQuery) -> Vec<LogEntry> {
        let buffers = self.buffers.lock();
        let Some(buf) = buffers.get(service_id) else {
            return Vec::new();
        };

        let iter = buf.iter().filter(|e| {
            if let Some(since) = query.since_ms {
                if e.timestamp_ms <= since {
                    return false;
                }
            }
            if let Some(ref level) = query.level {
                if !e.level.eq_ignore_ascii_case(level) {
                    return false;
                }
            }
            if let Some(ref search) = query.search {
                let needle = search.to_lowercase();
                if !e.message.to_lowercase().contains(&needle) {
                    return false;
                }
            }
            true
        });

        match query.limit {
            Some(limit) => {
                iter.rev().take(limit).cloned().collect::<Vec<_>>().into_iter().rev().collect()
            }
            None => iter.cloned().collect(),
        }
    }

    /// Return the list of known service ids.
    pub fn service_ids(&self) -> Vec<String> {
        self.buffers.lock().keys().cloned().collect()
    }

    /// Ensure a buffer exists for the given service id.
    pub fn ensure_buffer(&self, service_id: &str) {
        let mut buffers = self.buffers.lock();
        buffers
            .entry(service_id.to_string())
            .or_insert_with(|| VecDeque::with_capacity(self.capacity));
    }

    /// Push a log entry into the appropriate ring buffer.
    fn push(&self, service_id: &str, entry: LogEntry) {
        let mut buffers = self.buffers.lock();
        let buf = buffers
            .entry(service_id.to_string())
            .or_insert_with(|| VecDeque::with_capacity(self.capacity));
        if buf.len() >= self.capacity {
            buf.pop_front();
        }
        buf.push_back(entry);
    }
}

impl Default for ServiceLogCollector {
    fn default() -> Self {
        Self::new()
    }
}

// ── Tracing Layer ───────────────────────────────────────────────────

/// A `tracing_subscriber::Layer` that captures log events tagged with a
/// `service` field into per-service ring buffers.
///
/// To tag events, wrap work in a span:
/// ```ignore
/// let _guard = tracing::info_span!("service", service = "scheduler").entered();
/// tracing::info!("tick");
/// ```
impl<S> Layer<S> for ServiceLogCollector
where
    S: tracing::Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_new_span(&self, attrs: &span::Attributes<'_>, id: &span::Id, ctx: Context<'_, S>) {
        let mut visitor = ServiceFieldVisitor::default();
        attrs.record(&mut visitor);
        if let Some(service) = visitor.service {
            if let Some(span) = ctx.span(id) {
                span.extensions_mut().insert(ServiceId(service));
            }
        }
    }

    fn on_event(&self, event: &tracing::Event<'_>, ctx: Context<'_, S>) {
        // Walk from the current span upward to find the service id.
        let service_id = ctx.event_span(event).and_then(|span| {
            // Check self first, then parents.
            let extensions = span.extensions();
            if let Some(sid) = extensions.get::<ServiceId>() {
                return Some(sid.0.clone());
            }
            drop(extensions);
            span.scope()
                .skip(1)
                .find_map(|s| s.extensions().get::<ServiceId>().map(|sid| sid.0.clone()))
        });

        let Some(service_id) = service_id else {
            return;
        };

        let mut visitor = EventVisitor::default();
        event.record(&mut visitor);

        let now_ms =
            SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as u64;

        let entry = LogEntry {
            timestamp_ms: now_ms,
            level: event.metadata().level().to_string(),
            message: visitor.message,
            fields: visitor.fields,
            target: event.metadata().target().to_string(),
        };

        self.push(&service_id, entry);
    }
}

// ── Field visitors ──────────────────────────────────────────────────

/// Extracts the `service` field from span attributes.
#[derive(Default)]
struct ServiceFieldVisitor {
    service: Option<String>,
}

impl Visit for ServiceFieldVisitor {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "service" {
            self.service = Some(value.to_string());
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "service" {
            self.service = Some(format!("{:?}", value).trim_matches('"').to_string());
        }
    }
}

/// Extracts message and fields from an event.
#[derive(Default)]
struct EventVisitor {
    message: String,
    fields: HashMap<String, String>,
}

impl Visit for EventVisitor {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.message = value.to_string();
        } else {
            self.fields.insert(field.name().to_string(), value.to_string());
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        let formatted = format!("{:?}", value);
        if field.name() == "message" {
            self.message = formatted.trim_matches('"').to_string();
        } else {
            self.fields.insert(field.name().to_string(), formatted);
        }
    }
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tracing_subscriber::prelude::*;

    fn setup_collector() -> ServiceLogCollector {
        ServiceLogCollector::with_capacity(5)
    }

    #[test]
    fn push_and_query_basic() {
        let collector = setup_collector();
        collector.push(
            "sched",
            LogEntry {
                timestamp_ms: 100,
                level: "INFO".into(),
                message: "tick".into(),
                fields: HashMap::new(),
                target: "test".into(),
            },
        );
        let logs = collector.get_logs("sched", &LogQuery::default());
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].message, "tick");
    }

    #[test]
    fn ring_buffer_eviction() {
        let collector = setup_collector(); // capacity 5
        for i in 0..8 {
            collector.push(
                "svc",
                LogEntry {
                    timestamp_ms: i,
                    level: "INFO".into(),
                    message: format!("msg-{i}"),
                    fields: HashMap::new(),
                    target: "test".into(),
                },
            );
        }
        let logs = collector.get_logs("svc", &LogQuery::default());
        assert_eq!(logs.len(), 5);
        assert_eq!(logs[0].message, "msg-3");
        assert_eq!(logs[4].message, "msg-7");
    }

    #[test]
    fn query_since_filter() {
        let collector = setup_collector();
        for i in 1..=3 {
            collector.push(
                "svc",
                LogEntry {
                    timestamp_ms: i * 100,
                    level: "INFO".into(),
                    message: format!("m{i}"),
                    fields: HashMap::new(),
                    target: "test".into(),
                },
            );
        }
        let logs =
            collector.get_logs("svc", &LogQuery { since_ms: Some(100), ..Default::default() });
        assert_eq!(logs.len(), 2);
        assert_eq!(logs[0].timestamp_ms, 200);
    }

    #[test]
    fn query_level_filter() {
        let collector = setup_collector();
        collector.push(
            "svc",
            LogEntry {
                timestamp_ms: 1,
                level: "ERROR".into(),
                message: "bad".into(),
                fields: HashMap::new(),
                target: "test".into(),
            },
        );
        collector.push(
            "svc",
            LogEntry {
                timestamp_ms: 2,
                level: "INFO".into(),
                message: "ok".into(),
                fields: HashMap::new(),
                target: "test".into(),
            },
        );
        let logs = collector
            .get_logs("svc", &LogQuery { level: Some("ERROR".into()), ..Default::default() });
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].message, "bad");
    }

    #[test]
    fn query_search_filter() {
        let collector = setup_collector();
        collector.push(
            "svc",
            LogEntry {
                timestamp_ms: 1,
                level: "INFO".into(),
                message: "starting scheduler tick".into(),
                fields: HashMap::new(),
                target: "test".into(),
            },
        );
        collector.push(
            "svc",
            LogEntry {
                timestamp_ms: 2,
                level: "INFO".into(),
                message: "connected to server".into(),
                fields: HashMap::new(),
                target: "test".into(),
            },
        );
        let logs = collector
            .get_logs("svc", &LogQuery { search: Some("scheduler".into()), ..Default::default() });
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].message, "starting scheduler tick");
    }

    #[test]
    fn query_limit_returns_most_recent() {
        let collector = setup_collector();
        for i in 1..=5 {
            collector.push(
                "svc",
                LogEntry {
                    timestamp_ms: i,
                    level: "INFO".into(),
                    message: format!("m{i}"),
                    fields: HashMap::new(),
                    target: "test".into(),
                },
            );
        }
        let logs = collector.get_logs("svc", &LogQuery { limit: Some(2), ..Default::default() });
        assert_eq!(logs.len(), 2);
        // Most recent 2, in chronological order
        assert_eq!(logs[0].message, "m4");
        assert_eq!(logs[1].message, "m5");
    }

    #[test]
    fn tracing_layer_captures_events() {
        let collector = ServiceLogCollector::with_capacity(100);
        let collector_clone = collector.clone();

        let subscriber = tracing_subscriber::registry().with(collector_clone);
        let _guard = tracing::subscriber::set_default(subscriber);

        {
            let _span = tracing::info_span!("service", service = "test-svc").entered();
            tracing::info!("hello from service");
            tracing::warn!("a warning");
        }

        let logs = collector.get_logs("test-svc", &LogQuery::default());
        assert_eq!(logs.len(), 2);
        assert_eq!(logs[0].level, "INFO");
        assert!(logs[0].message.contains("hello from service"));
        assert_eq!(logs[1].level, "WARN");
    }

    #[test]
    fn events_without_service_span_are_ignored() {
        let collector = ServiceLogCollector::with_capacity(100);
        let collector_clone = collector.clone();

        let subscriber = tracing_subscriber::registry().with(collector_clone);
        let _guard = tracing::subscriber::set_default(subscriber);

        tracing::info!("no service span");

        assert!(collector.service_ids().is_empty());
    }

    #[test]
    fn unknown_service_returns_empty() {
        let collector = ServiceLogCollector::new();
        let logs = collector.get_logs("nonexistent", &LogQuery::default());
        assert!(logs.is_empty());
    }
}
