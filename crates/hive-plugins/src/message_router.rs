//! Plugin message router — routes incoming messages from plugins
//! into the host's connector pipeline.

use crate::protocol::IncomingMessage;
use parking_lot::Mutex;
use std::collections::HashSet;
use tracing::info;

/// Routes messages from plugins into the connector service pipeline.
pub struct PluginMessageRouter {
    /// Seen source IDs for deduplication.
    seen_sources: Mutex<HashSet<String>>,
    /// Maximum number of sources to track (LRU-style eviction at this limit).
    max_tracked: usize,
}

impl PluginMessageRouter {
    pub fn new() -> Self {
        Self { seen_sources: Mutex::new(HashSet::new()), max_tracked: 100_000 }
    }

    /// Process an incoming message from a plugin.
    /// Returns `true` if the message is new (not a duplicate).
    pub fn process_message(&self, msg: &IncomingMessage) -> bool {
        let mut seen = self.seen_sources.lock();

        // Dedup by source
        if seen.contains(&msg.source) {
            return false;
        }

        // Evict if at capacity
        if seen.len() >= self.max_tracked {
            seen.clear(); // Simple reset; a proper LRU would be better
        }

        seen.insert(msg.source.clone());

        info!(
            source = %msg.source,
            channel = %msg.channel,
            content_len = msg.content.len(),
            "Plugin message received"
        );

        true
    }
}

impl Default for PluginMessageRouter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_msg(source: &str) -> IncomingMessage {
        IncomingMessage {
            source: source.into(),
            channel: "test".into(),
            content: "hello".into(),
            sender: None,
            metadata: None,
            classification: None,
            thread_id: None,
            timestamp: None,
            attachments: None,
        }
    }

    #[test]
    fn test_dedup() {
        let router = PluginMessageRouter::new();
        let msg = make_msg("test:1");

        assert!(router.process_message(&msg));
        assert!(!router.process_message(&msg)); // duplicate
    }

    #[test]
    fn test_different_sources() {
        let router = PluginMessageRouter::new();

        assert!(router.process_message(&make_msg("a")));
        assert!(router.process_message(&make_msg("b")));
        assert!(!router.process_message(&make_msg("a"))); // dup
    }
}
