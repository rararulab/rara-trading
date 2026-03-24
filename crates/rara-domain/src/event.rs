//! Domain event model for the event bus.

use bon::Builder;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A domain event flowing through the event bus.
#[derive(Debug, Clone, Serialize, Deserialize, Builder)]
#[allow(clippy::struct_field_names)]
pub struct Event {
    /// Unique event identifier.
    #[builder(default = Uuid::new_v4())]
    event_id: Uuid,
    /// Dotted event type, e.g. `"trading.order.filled"`.
    #[builder(into)]
    event_type: String,
    /// When the event occurred.
    #[builder(default = jiff::Timestamp::now())]
    timestamp: jiff::Timestamp,
    /// System or component that produced the event.
    #[builder(into)]
    source: String,
    /// Correlation ID for tracing related events.
    #[builder(into)]
    correlation_id: String,
    /// Optional strategy that originated this event.
    strategy_id: Option<String>,
    /// Optional strategy version.
    strategy_version: Option<u32>,
    /// Arbitrary JSON payload.
    payload: serde_json::Value,
}

impl Event {
    /// Returns the unique event identifier.
    pub const fn event_id(&self) -> Uuid {
        self.event_id
    }

    /// Returns the dotted event type string.
    pub fn event_type(&self) -> &str {
        &self.event_type
    }

    /// Returns the event timestamp.
    pub const fn timestamp(&self) -> jiff::Timestamp {
        self.timestamp
    }

    /// Returns the source component.
    pub fn source(&self) -> &str {
        &self.source
    }

    /// Returns the correlation ID.
    pub fn correlation_id(&self) -> &str {
        &self.correlation_id
    }

    /// Returns the optional strategy identifier.
    pub fn strategy_id(&self) -> Option<&str> {
        self.strategy_id.as_deref()
    }

    /// Returns the optional strategy version.
    pub const fn strategy_version(&self) -> Option<u32> {
        self.strategy_version
    }

    /// Returns the event payload.
    pub const fn payload(&self) -> &serde_json::Value {
        &self.payload
    }

    /// Returns the topic — the first segment of the dotted event type.
    pub fn topic(&self) -> &str {
        self.event_type
            .split('.')
            .next()
            .unwrap_or(&self.event_type)
    }
}
