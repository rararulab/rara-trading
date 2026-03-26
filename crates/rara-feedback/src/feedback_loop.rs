//! Feedback loop — periodic evaluation of strategies via the consumer,
//! with lifecycle event publishing on promote/demote/retire decisions.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use bon::Builder;
use snafu::{ResultExt, Snafu};

use rara_domain::event::{Event, EventType};
use rara_domain::feedback::FeedbackDecision;
use rara_event_bus::bus::EventBus;
use rara_event_bus::store::StoreError;

use crate::consumer::{ConsumerError, FeedbackConsumer};
use crate::evaluator::StrategyEvaluator;

/// Errors from the feedback loop.
#[derive(Debug, Snafu)]
#[snafu(visibility(pub))]
pub enum FeedbackLoopError {
    /// Consumer poll failed.
    #[snafu(display("consumer error: {source}"))]
    Consumer {
        /// The underlying consumer error.
        source: ConsumerError,
    },
    /// Event bus publish failed.
    #[snafu(display("event bus publish error: {source}"))]
    Publish {
        /// The underlying store error.
        source: StoreError,
    },
}

/// Result type for feedback loop operations.
pub type Result<T> = std::result::Result<T, FeedbackLoopError>;

/// Configuration for the periodic feedback evaluation loop.
#[derive(Debug, Clone, Builder)]
pub struct FeedbackLoopConfig {
    /// Interval between evaluation ticks.
    #[builder(default = Duration::from_secs(3600))]
    pub eval_interval: Duration,
    /// Minimum new trades since last evaluation before re-evaluating a strategy.
    #[builder(default = 100)]
    pub min_trades_between_evals: u32,
}

impl Default for FeedbackLoopConfig {
    fn default() -> Self {
        Self {
            eval_interval: Duration::from_secs(3600),
            min_trades_between_evals: 100,
        }
    }
}

/// Publish a lifecycle event based on the evaluation decision.
///
/// Maps each [`FeedbackDecision`] to an [`EventType`] and publishes it to the
/// event bus with a metrics snapshot payload.
pub fn publish_decision_event(
    event_bus: &EventBus,
    strategy_id: &str,
    decision: FeedbackDecision,
    reason: &str,
    metrics: &rara_domain::feedback::StrategyMetrics,
) -> Result<u64> {
    let event_type = match decision {
        FeedbackDecision::Promote => EventType::FeedbackStrategyPromote,
        FeedbackDecision::Demote => EventType::FeedbackStrategyDemote,
        FeedbackDecision::Hold => EventType::FeedbackStrategyHold,
        FeedbackDecision::Retire => EventType::FeedbackResearchRetrainRequested,
    };

    let payload = serde_json::json!({
        "decision": decision.to_string(),
        "reason": reason,
        "sharpe_ratio": metrics.sharpe_ratio,
        "win_rate": metrics.win_rate,
        "trade_count": metrics.trade_count,
        "pnl": metrics.pnl.to_string(),
        "max_drawdown": metrics.max_drawdown.to_string(),
    });

    let event = Event::builder()
        .event_type(event_type)
        .source("feedback-loop")
        .correlation_id(uuid::Uuid::new_v4().to_string())
        .strategy_id(strategy_id.to_owned())
        .payload(payload)
        .build();

    event_bus.publish(&event).context(PublishSnafu)
}

/// Run one evaluation tick: poll the consumer, evaluate all strategies that
/// have accumulated enough new trades, and publish lifecycle events.
///
/// Returns the number of strategies that were evaluated.
pub fn evaluate_tick<S: std::hash::BuildHasher>(
    consumer: &mut FeedbackConsumer,
    evaluator: &StrategyEvaluator,
    event_bus: &EventBus,
    last_eval_trades: &mut HashMap<String, u32, S>,
    min_trades_between_evals: u32,
) -> Result<usize> {
    consumer.poll().context(ConsumerSnafu)?;

    let mut evaluated = 0;

    for (strategy_id, metrics) in consumer.all_metrics_with_ids() {
        let prev_trades = last_eval_trades.get(&strategy_id).copied().unwrap_or(0);
        let new_trades = metrics.trade_count.saturating_sub(prev_trades);

        if new_trades < min_trades_between_evals {
            continue;
        }

        let (decision, reason) = evaluator.evaluate(&metrics, false);

        tracing::info!(
            strategy = %strategy_id,
            decision = %decision,
            reason = %reason,
            "strategy evaluated"
        );

        publish_decision_event(event_bus, &strategy_id, decision, &reason, &metrics)?;

        last_eval_trades.insert(strategy_id, metrics.trade_count);
        evaluated += 1;
    }

    Ok(evaluated)
}

/// Run the feedback evaluation loop indefinitely.
///
/// Periodically polls the consumer for new trading events, evaluates
/// strategies against thresholds, and publishes lifecycle events for
/// promote/demote/retire decisions.
pub async fn run_feedback_loop(
    event_bus: Arc<EventBus>,
    evaluator: StrategyEvaluator,
    config: FeedbackLoopConfig,
) {
    let mut consumer = FeedbackConsumer::new(Arc::clone(&event_bus));
    let mut interval = tokio::time::interval(config.eval_interval);
    let mut last_eval_trades: HashMap<String, u32> = HashMap::new();

    tracing::info!(
        eval_interval_secs = config.eval_interval.as_secs(),
        min_trades = config.min_trades_between_evals,
        "feedback loop started"
    );

    loop {
        interval.tick().await;

        if let Err(e) = evaluate_tick(
            &mut consumer,
            &evaluator,
            &event_bus,
            &mut last_eval_trades,
            config.min_trades_between_evals,
        ) {
            tracing::error!(error = %e, "feedback loop tick failed");
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;

    use rust_decimal_macros::dec;
    use serde_json::json;

    use rara_domain::event::{Event, EventType};
    use rara_domain::feedback::FeedbackDecision;
    use rara_event_bus::bus::EventBus;

    use crate::evaluator::StrategyEvaluator;

    use super::*;

    fn setup() -> (Arc<EventBus>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let bus = Arc::new(EventBus::open(dir.path()).unwrap());
        (bus, dir)
    }

    fn publish_fill(bus: &EventBus, strategy_id: &str, realized_pnl: &str) {
        let event = Event::builder()
            .event_type(EventType::TradingOrderFilled)
            .source("test")
            .correlation_id("test-corr")
            .strategy_id(strategy_id.to_owned())
            .payload(json!({ "realized_pnl": realized_pnl }))
            .build();
        bus.publish(&event).unwrap();
    }

    fn default_evaluator() -> StrategyEvaluator {
        StrategyEvaluator::new(1.5, dec!(0.20), 5)
    }

    #[test]
    fn evaluate_tick_publishes_promote_on_strong_performance() {
        let (bus, _dir) = setup();

        // Publish enough winning trades to exceed min_trades and get promotion
        let pnls = ["120", "80", "110", "90", "130", "95", "105", "115", "100", "85"];
        for pnl in &pnls {
            publish_fill(&bus, "strat-1", pnl);
        }

        let mut consumer = FeedbackConsumer::new(Arc::clone(&bus));
        let evaluator = default_evaluator();
        let mut last_eval = HashMap::new();

        let evaluated = evaluate_tick(&mut consumer, &evaluator, &bus, &mut last_eval, 1).unwrap();

        assert_eq!(evaluated, 1);

        let feedback_events = bus.store().read_topic("feedback", 0, 10).unwrap();
        assert_eq!(feedback_events.len(), 1);
        assert_eq!(
            feedback_events[0].event_type,
            EventType::FeedbackStrategyPromote
        );
        assert_eq!(
            feedback_events[0].strategy_id.as_deref(),
            Some("strat-1")
        );
    }

    #[test]
    fn evaluate_tick_publishes_retrain_on_excessive_drawdown() {
        let (bus, _dir) = setup();

        // Publish enough losing trades to trigger high drawdown
        for _ in 0..10 {
            publish_fill(&bus, "strat-1", "-500");
        }

        let mut consumer = FeedbackConsumer::new(Arc::clone(&bus));
        let evaluator = default_evaluator();
        let mut last_eval = HashMap::new();

        evaluate_tick(&mut consumer, &evaluator, &bus, &mut last_eval, 1).unwrap();

        let feedback_events = bus.store().read_topic("feedback", 0, 10).unwrap();
        assert_eq!(feedback_events.len(), 1);
        assert_eq!(
            feedback_events[0].event_type,
            EventType::FeedbackResearchRetrainRequested
        );
    }

    #[test]
    fn evaluate_tick_holds_on_insufficient_trades() {
        let (bus, _dir) = setup();

        publish_fill(&bus, "strat-1", "100");

        let mut consumer = FeedbackConsumer::new(Arc::clone(&bus));
        let evaluator = default_evaluator();
        let mut last_eval = HashMap::new();

        evaluate_tick(&mut consumer, &evaluator, &bus, &mut last_eval, 1).unwrap();

        let feedback_events = bus.store().read_topic("feedback", 0, 10).unwrap();
        assert_eq!(feedback_events.len(), 1);
        assert_eq!(
            feedback_events[0].event_type,
            EventType::FeedbackStrategyHold
        );
    }

    #[test]
    fn evaluate_tick_skips_strategies_below_min_trades_between_evals() {
        let (bus, _dir) = setup();

        for _ in 0..10 {
            publish_fill(&bus, "strat-1", "100");
        }

        let mut consumer = FeedbackConsumer::new(Arc::clone(&bus));
        let evaluator = default_evaluator();
        let mut last_eval = HashMap::new();

        // First tick evaluates (10 new trades >= min 5)
        let evaluated =
            evaluate_tick(&mut consumer, &evaluator, &bus, &mut last_eval, 5).unwrap();
        assert_eq!(evaluated, 1);

        // Add 3 more trades (below min_trades_between_evals of 5)
        for _ in 0..3 {
            publish_fill(&bus, "strat-1", "100");
        }

        // Second tick should skip (only 3 new trades < min 5)
        let evaluated =
            evaluate_tick(&mut consumer, &evaluator, &bus, &mut last_eval, 5).unwrap();
        assert_eq!(evaluated, 0);

        // Only 1 feedback event from the first evaluation
        let feedback_events = bus.store().read_topic("feedback", 0, 10).unwrap();
        assert_eq!(feedback_events.len(), 1);
    }

    #[test]
    fn evaluate_tick_handles_multiple_strategies() {
        let (bus, _dir) = setup();

        // Two strategies, each with enough trades
        for _ in 0..6 {
            publish_fill(&bus, "alpha", "100");
            publish_fill(&bus, "beta", "-200");
        }

        let mut consumer = FeedbackConsumer::new(Arc::clone(&bus));
        let evaluator = default_evaluator();
        let mut last_eval = HashMap::new();

        let evaluated = evaluate_tick(&mut consumer, &evaluator, &bus, &mut last_eval, 1).unwrap();
        assert_eq!(evaluated, 2);

        let feedback_events = bus.store().read_topic("feedback", 0, 10).unwrap();
        assert_eq!(feedback_events.len(), 2);
    }

    #[test]
    fn evaluate_tick_re_evaluates_after_enough_new_trades() {
        let (bus, _dir) = setup();

        for _ in 0..10 {
            publish_fill(&bus, "strat-1", "100");
        }

        let mut consumer = FeedbackConsumer::new(Arc::clone(&bus));
        let evaluator = default_evaluator();
        let mut last_eval = HashMap::new();

        // First evaluation
        evaluate_tick(&mut consumer, &evaluator, &bus, &mut last_eval, 5).unwrap();

        // Add enough new trades
        for _ in 0..5 {
            publish_fill(&bus, "strat-1", "100");
        }

        // Second evaluation should trigger
        let evaluated =
            evaluate_tick(&mut consumer, &evaluator, &bus, &mut last_eval, 5).unwrap();
        assert_eq!(evaluated, 1);

        let feedback_events = bus.store().read_topic("feedback", 0, 10).unwrap();
        assert_eq!(feedback_events.len(), 2);
    }

    #[test]
    fn publish_decision_event_includes_correct_payload() {
        let (bus, _dir) = setup();

        let metrics = rara_domain::feedback::StrategyMetrics::builder()
            .pnl(dec!(1000))
            .sharpe_ratio(2.0)
            .max_drawdown(dec!(0.05))
            .win_rate(0.65)
            .trade_count(50)
            .build();

        publish_decision_event(&bus, "strat-1", FeedbackDecision::Promote, "good", &metrics)
            .unwrap();

        let events = bus.store().read_topic("feedback", 0, 10).unwrap();
        assert_eq!(events.len(), 1);

        let payload = &events[0].payload;
        assert_eq!(payload["decision"], "Promote");
        assert_eq!(payload["reason"], "good");
        assert_eq!(payload["trade_count"], 50);
    }

    #[test]
    fn demote_publishes_demotion_event_type() {
        let (bus, _dir) = setup();

        let metrics = rara_domain::feedback::StrategyMetrics::builder()
            .pnl(dec!(-500))
            .sharpe_ratio(0.1)
            .max_drawdown(dec!(0.15))
            .win_rate(0.3)
            .trade_count(30)
            .build();

        publish_decision_event(&bus, "strat-1", FeedbackDecision::Demote, "bad sharpe", &metrics)
            .unwrap();

        let events = bus.store().read_topic("feedback", 0, 10).unwrap();
        assert_eq!(events[0].event_type, EventType::FeedbackStrategyDemote);
    }
}
