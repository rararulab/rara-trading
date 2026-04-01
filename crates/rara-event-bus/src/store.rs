//! Sled-backed persistent event store with topic indexing.

use std::path::Path;

use rara_domain::event::Event;
use snafu::{ResultExt, Snafu};

/// Errors that can occur in the event store.
#[derive(Debug, Snafu)]
#[snafu(visibility(pub))]
pub enum StoreError {
    /// A sled database error.
    #[snafu(display("sled error: {source}"))]
    Sled {
        /// The underlying sled error.
        source: sled::Error,
    },
    /// A JSON serialization/deserialization error.
    #[snafu(display("serialization error: {source}"))]
    Serialize {
        /// The underlying `serde_json` error.
        source: serde_json::Error,
    },
}

/// Alias for results from event store operations.
pub type Result<T> = std::result::Result<T, StoreError>;

/// Persistent event store backed by sled, with topic-based indexing and
/// consumer offset tracking.
pub struct EventStore {
    /// Primary store: seq (u64 BE bytes) -> Event JSON bytes.
    events:  sled::Tree,
    /// Topic index: "{topic}/{seq:020}" -> seq bytes.
    topics:  sled::Tree,
    /// Consumer offsets: `"{consumer_id}/{topic}"` -> u64 BE bytes.
    offsets: sled::Tree,
    /// The underlying sled database, used for monotonic ID generation.
    db:      sled::Db,
}

impl EventStore {
    /// Open (or create) an event store at the given path.
    pub fn open(path: &Path) -> Result<Self> {
        let db = sled::open(path).context(SledSnafu)?;
        let events = db.open_tree("events").context(SledSnafu)?;
        let topics = db.open_tree("topics").context(SledSnafu)?;
        let offsets = db.open_tree("offsets").context(SledSnafu)?;
        Ok(Self {
            events,
            topics,
            offsets,
            db,
        })
    }

    /// Append an event to the store, returning its sequence number.
    ///
    /// The event is serialized to JSON and stored under a monotonically
    /// increasing sequence key. A topic index entry is also created so
    /// events can be queried by topic efficiently.
    pub fn append(&self, event: &Event) -> Result<u64> {
        let seq = self.db.generate_id().context(SledSnafu)?;
        let json = serde_json::to_vec(event).context(SerializeSnafu)?;

        self.events
            .insert(seq.to_be_bytes(), json)
            .context(SledSnafu)?;

        // Topic index key: "{topic}/{seq:020}" for lexicographic ordering
        let topic_key = format!("{}/{:020}", event.topic(), seq);
        self.topics
            .insert(topic_key.as_bytes(), &seq.to_be_bytes())
            .context(SledSnafu)?;

        Ok(seq)
    }

    /// Retrieve a single event by its sequence number.
    pub fn get(&self, seq: u64) -> Result<Option<Event>> {
        self.events
            .get(seq.to_be_bytes())
            .context(SledSnafu)?
            .map(|bytes| serde_json::from_slice(&bytes).context(SerializeSnafu))
            .transpose()
    }

    /// Read events for a topic starting from `from_seq`, returning at most
    /// `limit` events in sequence order.
    pub fn read_topic(&self, topic: &str, from_seq: u64, limit: usize) -> Result<Vec<Event>> {
        let start_key = format!("{topic}/{from_seq:020}");
        // Prefix is just the topic + slash, so the scan stays within this topic
        let prefix = format!("{topic}/");

        self.topics
            .range(start_key.as_bytes()..)
            .take_while(|res| {
                res.as_ref()
                    .map(|(k, _)| k.starts_with(prefix.as_bytes()))
                    .unwrap_or(true) // let errors propagate through
            })
            .take(limit)
            .map(|res| {
                let (_, seq_bytes) = res.context(SledSnafu)?;
                let seq = u64::from_be_bytes(
                    seq_bytes
                        .as_ref()
                        .try_into()
                        .expect("seq bytes must be 8 bytes"),
                );
                // Event must exist if the topic index references it
                Ok(self
                    .get(seq)?
                    .expect("topic index references non-existent event"))
            })
            .collect()
    }

    /// Set the consumer offset for a given consumer and topic.
    pub fn set_offset(&self, consumer_id: &str, topic: &str, offset: u64) -> Result<()> {
        let key = format!("{consumer_id}/{topic}");
        self.offsets
            .insert(key.as_bytes(), &offset.to_be_bytes())
            .context(SledSnafu)?;
        Ok(())
    }

    /// Get the consumer offset for a given consumer and topic, defaulting to 0.
    pub fn get_offset(&self, consumer_id: &str, topic: &str) -> Result<u64> {
        let key = format!("{consumer_id}/{topic}");
        let offset = self
            .offsets
            .get(key.as_bytes())
            .context(SledSnafu)?
            .map_or(0, |bytes| {
                u64::from_be_bytes(
                    bytes
                        .as_ref()
                        .try_into()
                        .expect("offset bytes must be 8 bytes"),
                )
            });
        Ok(offset)
    }
}

#[cfg(test)]
mod tests {
    use rara_domain::event::EventType;
    use serde_json::json;

    use super::*;

    #[test]
    fn append_and_read_event() {
        let dir = tempfile::tempdir().unwrap();
        let store = EventStore::open(dir.path()).unwrap();
        let event = Event::builder()
            .event_type(EventType::TradingOrderSubmitted)
            .source("test")
            .correlation_id("corr-1")
            .payload(json!({"msg": "hello"}))
            .build();
        let seq = store.append(&event).unwrap();
        let retrieved = store.get(seq).unwrap().unwrap();
        assert_eq!(retrieved.event_type, EventType::TradingOrderSubmitted);
    }

    #[test]
    fn read_by_topic_from_offset() {
        let dir = tempfile::tempdir().unwrap();
        let store = EventStore::open(dir.path()).unwrap();

        // Publish 2 trading events + 1 research event
        let trading1 = Event::builder()
            .event_type(EventType::TradingOrderSubmitted)
            .source("test")
            .correlation_id("corr-1")
            .payload(json!({"pair": "BTC/USD"}))
            .build();
        let trading2 = Event::builder()
            .event_type(EventType::TradingOrderFilled)
            .source("test")
            .correlation_id("corr-2")
            .payload(json!({"pair": "ETH/USD"}))
            .build();
        let research = Event::builder()
            .event_type(EventType::ResearchHypothesisCreated)
            .source("test")
            .correlation_id("corr-3")
            .payload(json!({"topic": "momentum"}))
            .build();

        store.append(&trading1).unwrap();
        store.append(&trading2).unwrap();
        store.append(&research).unwrap();

        let trading_events = store.read_topic("trading", 0, 10).unwrap();
        assert_eq!(trading_events.len(), 2);

        let research_events = store.read_topic("research", 0, 10).unwrap();
        assert_eq!(research_events.len(), 1);
    }

    #[test]
    fn consumer_offset_tracking() {
        let dir = tempfile::tempdir().unwrap();
        let store = EventStore::open(dir.path()).unwrap();

        // Default offset is 0
        assert_eq!(store.get_offset("worker-1", "trading").unwrap(), 0);

        // Set and retrieve
        store.set_offset("worker-1", "trading", 42).unwrap();
        assert_eq!(store.get_offset("worker-1", "trading").unwrap(), 42);

        // Different consumer has independent offset
        assert_eq!(store.get_offset("worker-2", "trading").unwrap(), 0);
    }
}
