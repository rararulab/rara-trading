//! Domain event model for the event bus.

use bon::Builder;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// All known domain event types flowing through the event bus.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    Serialize,
    Deserialize,
    strum::Display,
    strum::EnumString,
)]
#[strum(serialize_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum EventType {
    /// A trading order has been submitted to the broker.
    TradingOrderSubmitted,
    /// A trading order has been filled.
    TradingOrderFilled,
    /// A trading order was rejected.
    TradingOrderRejected,
    /// A trading order status update.
    TradingOrderUpdated,
    /// A sentinel signal was detected.
    SentinelSignalDetected,
    /// A strategy was promoted by feedback evaluation.
    FeedbackStrategyPromote,
    /// A strategy was demoted by feedback evaluation.
    FeedbackStrategyDemote,
    /// A strategy is on hold pending more data.
    FeedbackStrategyHold,
    /// A strategy was confirmed after paper trading validation.
    FeedbackStrategyConfirmed,
    /// Retraining has been requested for a degraded strategy.
    FeedbackResearchRetrainRequested,
    /// A new research hypothesis was created.
    ResearchHypothesisCreated,
    /// A research experiment has completed.
    ResearchExperimentCompleted,
    /// A strategy candidate emerged from research.
    ResearchStrategyCandidate,
}

impl EventType {
    /// Returns the top-level topic for event bus routing.
    pub const fn topic(&self) -> &'static str {
        match self {
            Self::TradingOrderSubmitted
            | Self::TradingOrderFilled
            | Self::TradingOrderRejected
            | Self::TradingOrderUpdated => "trading",
            Self::SentinelSignalDetected => "sentinel",
            Self::FeedbackStrategyPromote
            | Self::FeedbackStrategyDemote
            | Self::FeedbackStrategyHold
            | Self::FeedbackStrategyConfirmed
            | Self::FeedbackResearchRetrainRequested => "feedback",
            Self::ResearchHypothesisCreated
            | Self::ResearchExperimentCompleted
            | Self::ResearchStrategyCandidate => "research",
        }
    }
}

/// A domain event flowing through the event bus.
#[derive(Debug, Clone, Serialize, Deserialize, Builder)]
pub struct Event {
    /// Unique event identifier.
    #[builder(default = Uuid::new_v4())]
    pub event_id:         Uuid,
    /// The type of this event.
    pub event_type:       EventType,
    /// When the event occurred.
    #[builder(default = jiff::Timestamp::now())]
    pub timestamp:        jiff::Timestamp,
    /// System or component that produced the event.
    #[builder(into)]
    pub source:           String,
    /// Correlation ID for tracing related events.
    #[builder(into)]
    pub correlation_id:   String,
    /// Optional strategy that originated this event.
    pub strategy_id:      Option<String>,
    /// Optional strategy version.
    pub strategy_version: Option<u32>,
    /// Arbitrary JSON payload.
    pub payload:          serde_json::Value,
}

impl Event {
    /// Returns the top-level topic for event bus routing.
    pub const fn topic(&self) -> &'static str { self.event_type.topic() }
}
