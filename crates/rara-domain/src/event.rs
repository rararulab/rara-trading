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
    pub event_id: Uuid,
    /// Dotted event type, e.g. `"trading.order.filled"`.
    #[builder(into)]
    pub event_type: String,
    /// When the event occurred.
    #[builder(default = jiff::Timestamp::now())]
    pub timestamp: jiff::Timestamp,
    /// System or component that produced the event.
    #[builder(into)]
    pub source: String,
    /// Correlation ID for tracing related events.
    #[builder(into)]
    pub correlation_id: String,
    /// Optional strategy that originated this event.
    pub strategy_id: Option<String>,
    /// Optional strategy version.
    pub strategy_version: Option<u32>,
    /// Arbitrary JSON payload.
    pub payload: serde_json::Value,
}

impl Event {

    /// Returns the topic — the first segment of the dotted event type.
    pub fn topic(&self) -> &str {
        self.event_type
            .split('.')
            .next()
            .unwrap_or(&self.event_type)
    }
}
