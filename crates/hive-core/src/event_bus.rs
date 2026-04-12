use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::{broadcast, mpsc};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EventEnvelope {
    pub id: u64,
    pub topic: String,
    pub payload: Value,
    pub timestamp_ms: u128,
    pub source: String,
}

/// Trait for subscribers that receive events into a dedicated queue.
/// Unlike broadcast subscribers, queued subscribers never drop events due to
/// slow consumption — each gets its own backpressure-independent channel.
pub trait QueuedSubscriber: Send + Sync + 'static {
    /// Return `true` if this subscriber wants the given envelope.
    fn accept(&self, envelope: &EventEnvelope) -> bool;

    /// Deliver the envelope.  Must not block.
    fn send(&self, envelope: EventEnvelope);

    /// Return `false` when the subscriber's channel is closed (consumer
    /// dropped).  `publish()` periodically prunes dead subscribers.
    fn is_alive(&self) -> bool {
        true
    }
}

/// Checks whether `topic` matches a given `prefix` (exact or dot-separated child).
pub fn topic_matches_prefix(topic: &str, prefix: &str) -> bool {
    topic == prefix
        || topic.starts_with(prefix) && topic.as_bytes().get(prefix.len()) == Some(&b'.')
}

// ── In-memory queued subscription (mpsc) ────────────────────────────

struct MpscQueuedSubscriber {
    prefix: String,
    tx: mpsc::UnboundedSender<EventEnvelope>,
}

impl QueuedSubscriber for MpscQueuedSubscriber {
    fn accept(&self, envelope: &EventEnvelope) -> bool {
        self.prefix.is_empty() || topic_matches_prefix(&envelope.topic, &self.prefix)
    }

    fn send(&self, envelope: EventEnvelope) {
        let _ = self.tx.send(envelope);
    }

    fn is_alive(&self) -> bool {
        !self.tx.is_closed()
    }
}

/// Bounded queued subscriber that drops the newest event (with a warning)
/// when the channel is full, preventing unbounded memory growth.
struct BoundedQueuedSubscriber {
    prefix: String,
    tx: mpsc::Sender<EventEnvelope>,
    dropped: AtomicU64,
}

impl QueuedSubscriber for BoundedQueuedSubscriber {
    fn accept(&self, envelope: &EventEnvelope) -> bool {
        self.prefix.is_empty() || topic_matches_prefix(&envelope.topic, &self.prefix)
    }

    fn send(&self, envelope: EventEnvelope) {
        if let Err(_e) = self.tx.try_send(envelope) {
            let count = self.dropped.fetch_add(1, Ordering::Relaxed) + 1;
            // Log every Nth drop to avoid log spam.
            if count == 1 || count % 100 == 0 {
                tracing::warn!(
                    prefix = %self.prefix,
                    total_dropped = count,
                    "event bus subscriber queue full, dropping event"
                );
            }
        }
    }

    fn is_alive(&self) -> bool {
        !self.tx.is_closed()
    }
}

// ── EventBus ────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct EventBus {
    sender: broadcast::Sender<EventEnvelope>,
    queued: Arc<RwLock<Vec<Arc<dyn QueuedSubscriber>>>>,
    next_id: Arc<AtomicU64>,
}

impl std::fmt::Debug for EventBus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EventBus")
            .field("queued_subscribers", &self.queued.read().len())
            .field("next_id", &self.next_id.load(Ordering::Relaxed))
            .finish()
    }
}

impl EventBus {
    pub fn new(capacity: usize) -> Self {
        let (sender, _) = broadcast::channel(capacity);
        Self {
            sender,
            queued: Arc::new(RwLock::new(Vec::new())),
            next_id: Arc::new(AtomicU64::new(1)),
        }
    }

    /// Advance the internal ID counter so the next published event will have
    /// an ID of at least `min_next`.  Used to seed the counter from the
    /// persisted event log on daemon restart so new event IDs don't collide
    /// with previously stored events.
    pub fn set_next_id(&self, min_next: u64) {
        self.next_id.fetch_max(min_next, Ordering::Relaxed);
    }

    pub fn publish(
        &self,
        topic: impl Into<String>,
        source: impl Into<String>,
        payload: Value,
    ) -> Result<usize, broadcast::error::SendError<EventEnvelope>> {
        let envelope = EventEnvelope {
            id: self.next_id.fetch_add(1, Ordering::Relaxed),
            topic: topic.into(),
            payload,
            timestamp_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis(),
            source: source.into(),
        };

        // Fan out to queued subscribers (lossless).
        // After delivery, prune any subscribers whose channels have closed.
        let mut queued_delivered = false;
        {
            let mut needs_prune = false;
            {
                let subs = self.queued.read();
                for sub in subs.iter() {
                    if sub.is_alive() {
                        if sub.accept(&envelope) {
                            sub.send(envelope.clone());
                            queued_delivered = true;
                        }
                    } else {
                        needs_prune = true;
                    }
                }
            }
            if needs_prune {
                self.queued.write().retain(|s| s.is_alive());
            }
        }

        // Broadcast (lossy). If queued subscribers already received
        // the event, ignore broadcast errors (zero receivers is fine).
        match self.sender.send(envelope) {
            Ok(n) => Ok(n),
            Err(_) if queued_delivered => Ok(0),
            Err(e) => Err(e),
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<EventEnvelope> {
        self.sender.subscribe()
    }

    pub fn subscribe_topic(&self, prefix: impl Into<String>) -> TopicSubscription {
        TopicSubscription { prefix: prefix.into(), receiver: self.subscribe() }
    }

    /// Create a lossless, per-subscriber queue filtered by topic prefix.
    /// Returns an unbounded receiver — events are never dropped.
    pub fn subscribe_queued(
        &self,
        prefix: impl Into<String>,
    ) -> mpsc::UnboundedReceiver<EventEnvelope> {
        let (tx, rx) = mpsc::unbounded_channel();
        let sub = Arc::new(MpscQueuedSubscriber { prefix: prefix.into(), tx });
        self.queued.write().push(sub);
        rx
    }

    /// Create a bounded, per-subscriber queue filtered by topic prefix.
    /// When the queue is full, new events are dropped (with a warning log).
    /// Default capacity is 10 000 events.
    pub fn subscribe_queued_bounded(
        &self,
        prefix: impl Into<String>,
        capacity: usize,
    ) -> mpsc::Receiver<EventEnvelope> {
        let (tx, rx) = mpsc::channel(capacity);
        let sub = Arc::new(BoundedQueuedSubscriber {
            prefix: prefix.into(),
            tx,
            dropped: AtomicU64::new(0),
        });
        self.queued.write().push(sub);
        rx
    }

    /// Register a custom queued subscriber (e.g. persistent event log).
    pub fn register_subscriber(&self, subscriber: Arc<dyn QueuedSubscriber>) {
        self.queued.write().push(subscriber);
    }

    /// Remove all dead (closed-channel) subscribers immediately.
    /// This is called lazily during `publish()`, but can be called explicitly
    /// to free resources sooner.
    pub fn prune_dead_subscribers(&self) -> usize {
        let mut subs = self.queued.write();
        let before = subs.len();
        subs.retain(|s| s.is_alive());
        before - subs.len()
    }

    /// Return the number of currently registered queued subscribers.
    pub fn queued_subscriber_count(&self) -> usize {
        self.queued.read().len()
    }
}

pub struct TopicSubscription {
    prefix: String,
    receiver: broadcast::Receiver<EventEnvelope>,
}

impl TopicSubscription {
    pub async fn recv(&mut self) -> Result<EventEnvelope, broadcast::error::RecvError> {
        loop {
            let event = self.receiver.recv().await?;
            if topic_matches_prefix(&event.topic, &self.prefix) {
                return Ok(event);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tokio::time::{timeout, Duration};

    #[tokio::test]
    async fn subscription_filters_by_prefix() {
        let bus = EventBus::new(16);
        let mut sub = bus.subscribe_topic("daemon");

        bus.publish("config.changed", "test", json!({"value": 1})).expect("publish config event");
        bus.publish("daemon.started", "test", json!({"value": 2})).expect("publish daemon event");

        let event = timeout(Duration::from_secs(1), sub.recv())
            .await
            .expect("event should arrive")
            .expect("subscription should succeed");

        assert_eq!(event.topic, "daemon.started");
        assert_eq!(event.payload["value"], 2);
    }

    #[tokio::test]
    async fn queued_subscription_no_message_loss() {
        let bus = EventBus::new(4); // tiny broadcast buffer
        let mut rx = bus.subscribe_queued("test");

        // Publish more events than broadcast capacity — queued must not drop.
        for i in 0..100 {
            let _ = bus.publish("test.event", "bench", json!({ "i": i }));
        }

        for i in 0..100 {
            let envelope =
                timeout(Duration::from_secs(1), rx.recv()).await.unwrap().expect("should receive");
            assert_eq!(envelope.payload["i"], i);
        }
    }

    #[tokio::test]
    async fn queued_subscription_filters_by_prefix() {
        let bus = EventBus::new(16);
        let mut rx = bus.subscribe_queued("chat");

        let _ = bus.publish("config.changed", "test", json!({}));
        let _ = bus.publish("chat.session.created", "test", json!({"sid": "s1"}));
        let _ = bus.publish("chat.message.queued", "test", json!({"sid": "s1"}));
        let _ = bus.publish("daemon.stopped", "test", json!({}));

        let e1 = timeout(Duration::from_secs(1), rx.recv()).await.unwrap().unwrap();
        assert_eq!(e1.topic, "chat.session.created");
        let e2 = timeout(Duration::from_secs(1), rx.recv()).await.unwrap().unwrap();
        assert_eq!(e2.topic, "chat.message.queued");

        // No more events expected
        assert!(timeout(Duration::from_millis(50), rx.recv()).await.is_err());
    }

    #[tokio::test]
    async fn queued_empty_prefix_receives_all() {
        let bus = EventBus::new(16);
        let mut rx = bus.subscribe_queued("");

        let _ = bus.publish("a", "test", json!({}));
        let _ = bus.publish("b.c", "test", json!({}));

        let e1 = timeout(Duration::from_secs(1), rx.recv()).await.unwrap().unwrap();
        assert_eq!(e1.topic, "a");
        let e2 = timeout(Duration::from_secs(1), rx.recv()).await.unwrap().unwrap();
        assert_eq!(e2.topic, "b.c");
    }

    #[tokio::test]
    async fn multiple_queued_subscribers() {
        let bus = EventBus::new(16);
        let mut rx_chat = bus.subscribe_queued("chat");
        let mut rx_mcp = bus.subscribe_queued("mcp");

        let _ = bus.publish("chat.created", "test", json!({}));
        let _ = bus.publish("mcp.connected", "test", json!({}));

        let e = timeout(Duration::from_secs(1), rx_chat.recv()).await.unwrap().unwrap();
        assert_eq!(e.topic, "chat.created");
        assert!(timeout(Duration::from_millis(50), rx_chat.recv()).await.is_err());

        let e = timeout(Duration::from_secs(1), rx_mcp.recv()).await.unwrap().unwrap();
        assert_eq!(e.topic, "mcp.connected");
        assert!(timeout(Duration::from_millis(50), rx_mcp.recv()).await.is_err());
    }

    #[tokio::test]
    async fn custom_queued_subscriber() {
        struct CountingSub {
            count: std::sync::atomic::AtomicUsize,
        }
        impl QueuedSubscriber for CountingSub {
            fn accept(&self, _: &EventEnvelope) -> bool {
                true
            }
            fn send(&self, _: EventEnvelope) {
                self.count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
        }

        let bus = EventBus::new(16);
        let counter = Arc::new(CountingSub { count: std::sync::atomic::AtomicUsize::new(0) });
        bus.register_subscriber(Arc::clone(&counter) as Arc<dyn QueuedSubscriber>);

        for _ in 0..10 {
            let _ = bus.publish("any.topic", "test", json!({}));
        }

        assert_eq!(counter.count.load(std::sync::atomic::Ordering::Relaxed), 10);
    }

    #[test]
    fn topic_prefix_matching() {
        assert!(topic_matches_prefix("daemon.started", "daemon"));
        assert!(topic_matches_prefix("daemon", "daemon"));
        assert!(topic_matches_prefix("chat.session.created", "chat"));
        assert!(topic_matches_prefix("chat.session.created", "chat.session"));
        assert!(!topic_matches_prefix("chatbot.started", "chat"));
        assert!(!topic_matches_prefix("config.changed", "daemon"));
    }

    #[tokio::test]
    async fn dead_subscribers_are_pruned() {
        let bus = EventBus::new(16);

        // Create two queued subscriptions.
        let rx1 = bus.subscribe_queued("topic");
        let _rx2 = bus.subscribe_queued("topic");

        assert_eq!(bus.queued.read().len(), 2);

        // Drop rx1 — its MpscQueuedSubscriber should be pruned on next publish.
        drop(rx1);

        let _ = bus.publish("topic.a", "test", json!({}));

        assert_eq!(bus.queued.read().len(), 1, "dead subscriber should be pruned");
    }
}
