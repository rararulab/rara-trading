//! Hybrid push+pull event bus combining persistent storage with broadcast
//! notifications.

use std::path::Path;

use tokio::sync::broadcast;

use rara_domain::event::Event;

use crate::store::{EventStore, Result};

/// A hybrid push+pull event bus.
///
/// Events are durably stored via [`EventStore`] and subscribers are notified
/// in real-time via a broadcast channel carrying sequence numbers.
/// Consumers that fall behind can catch up by reading directly from the store.
pub struct EventBus {
    /// Persistent backing store.
    store: EventStore,
    /// Broadcast sender for real-time sequence number notifications.
    tx: broadcast::Sender<u64>,
}

impl EventBus {
    /// Open (or create) an event bus backed by a sled database at the given path.
    pub fn open(path: &Path) -> Result<Self> {
        let store = EventStore::open(path)?;
        let (tx, _) = broadcast::channel(1024);
        Ok(Self { store, tx })
    }

    /// Publish an event: persist it and broadcast the sequence number to
    /// online subscribers.
    pub fn publish(&self, event: &Event) -> Result<u64> {
        let seq = self.store.append(event)?;
        // Ignore send error — it just means no active subscribers
        let _ = self.tx.send(seq);
        Ok(seq)
    }

    /// Subscribe to real-time sequence number notifications.
    pub fn subscribe(&self) -> broadcast::Receiver<u64> {
        self.tx.subscribe()
    }

    /// Access the underlying store for catch-up reads and offset management.
    pub const fn store(&self) -> &EventStore {
        &self.store
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[tokio::test]
    async fn publish_and_subscribe_receives_event() {
        let dir = tempfile::tempdir().unwrap();
        let bus = EventBus::open(dir.path()).unwrap();
        let mut rx = bus.subscribe();

        let event = Event::builder()
            .event_type("test.ping")
            .source("test")
            .correlation_id("corr-1")
            .payload(json!({"msg": "hello"}))
            .build();

        let seq = bus.publish(&event).unwrap();
        let received_seq = rx.recv().await.unwrap();
        assert_eq!(received_seq, seq);

        let stored = bus.store().get(seq).unwrap().unwrap();
        assert_eq!(stored.event_type(), "test.ping");
    }

    #[tokio::test]
    async fn catch_up_reads_missed_events() {
        let dir = tempfile::tempdir().unwrap();
        let bus = EventBus::open(dir.path()).unwrap();

        // Publish 3 events before anyone subscribes
        for i in 0..3 {
            let event = Event::builder()
                .event_type("trading.tick")
                .source("test")
                .correlation_id(format!("corr-{i}"))
                .payload(json!({"i": i}))
                .build();
            bus.publish(&event).unwrap();
        }

        // Late subscriber catches up via store
        let events = bus.store().read_topic("trading", 0, 10).unwrap();
        assert_eq!(events.len(), 3);
    }
}
