//! `FeedbackBridge` engine — orchestrates the strategy evaluation feedback loop.

use std::sync::Arc;

use snafu::{ResultExt, Snafu};
use uuid::Uuid;

use rara_domain::event::Event;
use rara_domain::feedback::{FeedbackDecision, StrategyReport};
use rara_event_bus::bus::EventBus;
use rara_event_bus::store::StoreError;

use crate::aggregator::{AggregatorError, MetricsAggregator};
use crate::evaluator::StrategyEvaluator;

/// Errors from feedback bridge operations.
#[derive(Debug, Snafu)]
#[snafu(visibility(pub))]
pub enum FeedbackBridgeError {
    /// Metrics aggregation failed.
    #[snafu(display("aggregation error: {source}"))]
    Aggregation {
        /// The underlying aggregator error.
        source: AggregatorError,
    },
    /// Failed to read or publish events.
    #[snafu(display("event bus error: {source}"))]
    EventBus {
        /// The underlying store error.
        source: StoreError,
    },
}

/// Alias for feedback bridge results.
pub type Result<T> = std::result::Result<T, FeedbackBridgeError>;

/// Orchestrates the feedback loop: aggregate metrics, evaluate the strategy,
/// and publish lifecycle events.
pub struct FeedbackBridge {
    aggregator: MetricsAggregator,
    evaluator: StrategyEvaluator,
    event_bus: Arc<EventBus>,
}

impl FeedbackBridge {
    /// Create a new feedback bridge.
    pub const fn new(
        aggregator: MetricsAggregator,
        evaluator: StrategyEvaluator,
        event_bus: Arc<EventBus>,
    ) -> Self {
        Self {
            aggregator,
            evaluator,
            event_bus,
        }
    }

    /// Evaluate a strategy over the given time window and publish the
    /// appropriate lifecycle event.
    ///
    /// Flow:
    /// 1. Aggregate trading metrics for the strategy
    /// 2. Check sentinel events for critical severity
    /// 3. Evaluate metrics + sentinel context → decision
    /// 4. Publish feedback event based on decision
    /// 5. Return the full strategy report
    pub fn evaluate_strategy(
        &self,
        strategy_id: &str,
        strategy_version: u32,
        window_start: jiff::Timestamp,
        window_end: jiff::Timestamp,
        sentinel_event_ids: Vec<Uuid>,
    ) -> Result<StrategyReport> {
        // 1. Aggregate metrics
        let metrics = self
            .aggregator
            .aggregate(strategy_id, window_start, window_end)
            .context(AggregationSnafu)?;

        // 2. Check for critical sentinel events
        let has_critical = self.has_critical_sentinel(&sentinel_event_ids)?;

        // 3. Evaluate
        let (decision, reason) = self.evaluator.evaluate(&metrics, has_critical);

        // 4. Build report
        let report = StrategyReport::builder()
            .strategy_id(strategy_id)
            .strategy_version(strategy_version)
            .window_start(window_start)
            .window_end(window_end)
            .metrics(metrics)
            .sentinel_events(sentinel_event_ids)
            .decision(decision)
            .reason(reason)
            .build();

        // 5. Publish lifecycle event
        let event_type = match decision {
            FeedbackDecision::Promote => "feedback.strategy.promote",
            FeedbackDecision::Demote => "feedback.strategy.demote",
            FeedbackDecision::Hold => "feedback.strategy.hold",
            FeedbackDecision::Retire => "feedback.research.retrain.requested",
        };

        let event = Event::builder()
            .event_type(event_type)
            .source("feedback-bridge")
            .correlation_id(uuid::Uuid::new_v4().to_string())
            .strategy_id(strategy_id.to_owned())
            .strategy_version(strategy_version)
            .payload(serde_json::json!({
                "decision": decision,
                "strategy_id": strategy_id,
                "strategy_version": strategy_version,
            }))
            .build();

        self.event_bus.publish(&event).context(EventBusSnafu)?;

        Ok(report)
    }

    /// Check whether any of the given sentinel event IDs have Critical severity.
    fn has_critical_sentinel(&self, event_ids: &[Uuid]) -> Result<bool> {
        let sentinel_events = self
            .event_bus
            .store()
            .read_topic("sentinel", 0, usize::MAX)
            .context(EventBusSnafu)?;

        let has_critical = sentinel_events.iter().any(|e| {
            event_ids.contains(&e.event_id())
                && e.payload()
                    .get("severity")
                    .and_then(|v| v.as_str())
                    == Some("Critical")
        });

        Ok(has_critical)
    }
}

#[cfg(test)]
mod tests {
    use rust_decimal_macros::dec;
    use serde_json::json;

    use super::*;

    fn setup() -> (Arc<EventBus>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let bus = Arc::new(EventBus::open(dir.path()).unwrap());
        (bus, dir)
    }

    fn bridge(event_bus: Arc<EventBus>) -> FeedbackBridge {
        let aggregator = MetricsAggregator::new(Arc::clone(&event_bus));
        let evaluator = StrategyEvaluator::new(1.5, dec!(0.20), 5);
        FeedbackBridge::new(aggregator, evaluator, event_bus)
    }

    fn publish_fill(bus: &EventBus, strategy_id: &str, realized_pnl: &str) -> Uuid {
        let event = Event::builder()
            .event_type("trading.order.filled")
            .source("test")
            .correlation_id("test-corr")
            .strategy_id(strategy_id.to_owned())
            .payload(json!({ "realized_pnl": realized_pnl }))
            .build();
        let id = event.event_id();
        bus.publish(&event).unwrap();
        id
    }

    fn publish_sentinel(bus: &EventBus, severity: &str) -> Uuid {
        let event = Event::builder()
            .event_type("sentinel.signal.detected")
            .source("test")
            .correlation_id("test-corr")
            .payload(json!({ "severity": severity }))
            .build();
        let id = event.event_id();
        bus.publish(&event).unwrap();
        id
    }

    fn window() -> (jiff::Timestamp, jiff::Timestamp) {
        let start = jiff::Timestamp::from_millisecond(0).unwrap();
        let end = jiff::Timestamp::now();
        (start, end)
    }

    #[test]
    fn promote_event_on_strong_performance() {
        let (bus, _dir) = setup();

        // Publish winning trades with slight variance so Sharpe > 0
        let pnls = ["120", "80", "110", "90", "130", "95", "105", "115", "100", "85"];
        for pnl in &pnls {
            publish_fill(&bus, "strat-1", pnl);
        }

        let fb = bridge(Arc::clone(&bus));
        let (start, end) = window();
        let report = fb
            .evaluate_strategy("strat-1", 1, start, end, vec![])
            .unwrap();

        assert_eq!(report.decision(), FeedbackDecision::Promote);

        // Verify feedback event was published
        let feedback_events = bus.store().read_topic("feedback", 0, 10).unwrap();
        assert_eq!(feedback_events.len(), 1);
        assert_eq!(
            feedback_events[0].event_type(),
            "feedback.strategy.promote"
        );
    }

    #[test]
    fn demote_on_critical_sentinel() {
        let (bus, _dir) = setup();

        let pnls = ["120", "80", "110", "90", "130", "95", "105", "115", "100", "85"];
        for pnl in &pnls {
            publish_fill(&bus, "strat-1", pnl);
        }

        let sentinel_id = publish_sentinel(&bus, "Critical");

        let fb = bridge(Arc::clone(&bus));
        let (start, end) = window();
        let report = fb
            .evaluate_strategy("strat-1", 1, start, end, vec![sentinel_id])
            .unwrap();

        assert_eq!(report.decision(), FeedbackDecision::Demote);
    }

    #[test]
    fn hold_on_insufficient_trades() {
        let (bus, _dir) = setup();

        publish_fill(&bus, "strat-1", "100");

        let fb = bridge(Arc::clone(&bus));
        let (start, end) = window();
        let report = fb
            .evaluate_strategy("strat-1", 1, start, end, vec![])
            .unwrap();

        assert_eq!(report.decision(), FeedbackDecision::Hold);
    }

    #[test]
    fn retire_on_retrain_requested() {
        let (bus, _dir) = setup();

        // Publish enough losing trades to trigger high drawdown
        for _ in 0..10 {
            publish_fill(&bus, "strat-1", "-500");
        }

        let fb = bridge(Arc::clone(&bus));
        let (start, end) = window();
        let report = fb
            .evaluate_strategy("strat-1", 1, start, end, vec![])
            .unwrap();

        assert_eq!(report.decision(), FeedbackDecision::Retire);

        let feedback_events = bus.store().read_topic("feedback", 0, 10).unwrap();
        assert_eq!(feedback_events.len(), 1);
        assert_eq!(
            feedback_events[0].event_type(),
            "feedback.research.retrain.requested"
        );
    }
}
