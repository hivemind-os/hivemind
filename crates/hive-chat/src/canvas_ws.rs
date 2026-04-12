use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};
use tokio::sync::broadcast;

/// Server → Client messages
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[allow(clippy::large_enum_variant)]
pub enum ServerMessage {
    Welcome { client_id: String, sequence: u64 },
    CanvasEvent { event: hive_canvas::CanvasEvent, sequence: u64, timestamp: u64 },
    Replay { events: Vec<SequencedEvent> },
    Error { message: String },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SequencedEvent {
    pub event: hive_canvas::CanvasEvent,
    pub sequence: u64,
    pub timestamp: u64,
}

// ---------------------------------------------------------------------------
// Per-session state
// ---------------------------------------------------------------------------

/// Maximum number of events retained in the in-memory event log per session.
const MAX_EVENT_LOG_SIZE: usize = 10_000;

/// Per-session state for canvas WebSocket connections.
pub struct CanvasSession {
    pub session_id: String,
    pub event_log: parking_lot::Mutex<VecDeque<SequencedEvent>>,
    pub sequence: AtomicU64,
    pub broadcast_tx: broadcast::Sender<ServerMessage>,
}

impl CanvasSession {
    pub fn new(session_id: String) -> Self {
        let (broadcast_tx, _) = broadcast::channel(256);
        Self {
            session_id,
            event_log: parking_lot::Mutex::new(VecDeque::new()),
            sequence: AtomicU64::new(0),
            broadcast_tx,
        }
    }

    /// Push a canvas event to all connected clients.
    pub fn push_event(&self, event: hive_canvas::CanvasEvent) {
        let seq = self.sequence.fetch_add(1, Ordering::Relaxed);
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        let sequenced = SequencedEvent { event: event.clone(), sequence: seq, timestamp };

        {
            let mut log = self.event_log.lock();
            if log.len() >= MAX_EVENT_LOG_SIZE {
                // Evict oldest 25% to amortise eviction overhead
                let drain_count = MAX_EVENT_LOG_SIZE / 4;
                drop(log.drain(..drain_count));
            }
            log.push_back(sequenced);
        }

        if let Err(e) =
            self.broadcast_tx.send(ServerMessage::CanvasEvent { event, sequence: seq, timestamp })
        {
            tracing::warn!(error = %e, "failed to broadcast canvas event");
        }
    }

    /// Get events after a given sequence number (for replay on reconnect).
    pub fn replay_from(&self, last_sequence: u64) -> Vec<SequencedEvent> {
        let log = self.event_log.lock();
        log.iter().filter(|e| e.sequence > last_sequence).cloned().collect()
    }
}

// ---------------------------------------------------------------------------
// Session registry
// ---------------------------------------------------------------------------

/// Registry of active canvas sessions keyed by session id.
#[derive(Clone)]
pub struct CanvasSessionRegistry {
    sessions: Arc<DashMap<String, Arc<CanvasSession>>>,
}

impl Default for CanvasSessionRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl CanvasSessionRegistry {
    pub fn new() -> Self {
        Self { sessions: Arc::new(DashMap::new()) }
    }

    pub fn get_or_create(&self, session_id: &str) -> Arc<CanvasSession> {
        self.sessions
            .entry(session_id.to_string())
            .or_insert_with(|| Arc::new(CanvasSession::new(session_id.to_string())))
            .value()
            .clone()
    }

    #[allow(dead_code)]
    pub fn get(&self, session_id: &str) -> Option<Arc<CanvasSession>> {
        self.sessions.get(session_id).map(|v| v.value().clone())
    }

    /// Remove a session from the registry, releasing its memory.
    pub fn remove(&self, session_id: &str) {
        self.sessions.remove(session_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_push_and_replay() {
        let session = CanvasSession::new("test".to_string());

        // Push two events
        session.push_event(hive_canvas::CanvasEvent::NodeUpdated {
            node_id: "n1".into(),
            patch: hive_canvas::NodePatch {
                content: None,
                status: None,
                x: Some(10.0),
                y: Some(20.0),
            },
        });
        session.push_event(hive_canvas::CanvasEvent::NodeUpdated {
            node_id: "n2".into(),
            patch: hive_canvas::NodePatch {
                content: None,
                status: None,
                x: Some(30.0),
                y: Some(40.0),
            },
        });

        assert_eq!(session.sequence.load(Ordering::Relaxed), 2);

        // Replay from 0 should return event with seq=1
        let replay = session.replay_from(0);
        assert_eq!(replay.len(), 1);
        assert_eq!(replay[0].sequence, 1);

        // Replay from u64::MAX should return nothing
        let replay = session.replay_from(u64::MAX);
        assert!(replay.is_empty());
    }

    #[test]
    fn registry_get_or_create() {
        let registry = CanvasSessionRegistry::new();
        let s1 = registry.get_or_create("sess-1");
        let s2 = registry.get_or_create("sess-1");
        // Same session returned
        assert_eq!(s1.session_id, s2.session_id);
        assert_eq!(Arc::as_ptr(&s1), Arc::as_ptr(&s2),);
    }

    #[test]
    fn server_message_serialization() {
        let msg = ServerMessage::Welcome { client_id: "c1".into(), sequence: 42 };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"type\":\"welcome\""));
        assert!(json.contains("\"client_id\":\"c1\""));

        let canvas_msg = ServerMessage::CanvasEvent {
            event: hive_canvas::CanvasEvent::NodeUpdated {
                node_id: "n1".into(),
                patch: hive_canvas::NodePatch {
                    content: None,
                    status: None,
                    x: Some(5.0),
                    y: None,
                },
            },
            sequence: 1,
            timestamp: 1000,
        };
        let json = serde_json::to_string(&canvas_msg).unwrap();
        assert!(json.contains("\"type\":\"canvas_event\""));
    }
}
