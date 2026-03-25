//! Retrain evaluation — compares paper trading performance against original
//! backtest results and triggers retraining when a promoted strategy degrades.

use std::sync::Arc;

use bon::Builder;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use snafu::{ResultExt, Snafu};
use uuid::Uuid;

use rara_domain::event::Event;
use rara_event_bus::bus::EventBus;
use rara_event_bus::store::StoreError;

/// Errors from retrain evaluation.
#[derive(Debug, Snafu)]
#[snafu(visibility(pub))]
pub enum RetrainError {
    /// Failed to publish an event to the bus.
    #[snafu(display("event bus error: {source}"))]
    EventBus {
        /// The underlying store error.
        source: StoreError,
    },
}

/// Alias for retrain results.
pub type Result<T> = std::result::Result<T, RetrainError>;

/// Outcome of a retrain evaluation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RetrainDecision {
    /// The strategy performs within acceptable bounds — confirmed for live use.
    Confirmed,
    /// The strategy has degraded — retraining has been requested.
    RetrainRequested {
        /// Human-readable explanation of why retraining was triggered.
        reason: String,
    },
}

/// Aggregated metrics from a paper trading session used to evaluate strategy
/// health before confirming or requesting retraining.
#[derive(Debug, Clone, Serialize, Deserialize, Builder)]
pub struct PaperMetrics {
    /// Sharpe ratio observed during paper trading.
    sharpe_ratio: f64,
    /// Maximum drawdown observed during paper trading.
    max_drawdown: Decimal,
    /// Total profit and loss (`PnL`).
    pnl: Decimal,
    /// Win rate as a fraction (0.0 – 1.0).
    win_rate: f64,
    /// Total number of trades executed.
    trade_count: u32,
}

impl PaperMetrics {
    /// Returns the Sharpe ratio.
    pub const fn sharpe_ratio(&self) -> f64 {
        self.sharpe_ratio
    }

    /// Returns the maximum drawdown.
    pub const fn max_drawdown(&self) -> Decimal {
        self.max_drawdown
    }

    /// Returns the total `PnL`.
    pub const fn pnl(&self) -> Decimal {
        self.pnl
    }

    /// Returns the win rate.
    pub const fn win_rate(&self) -> f64 {
        self.win_rate
    }

    /// Returns the trade count.
    pub const fn trade_count(&self) -> u32 {
        self.trade_count
    }
}

/// Metrics from the original backtest that serve as the baseline for
/// degradation comparison.
#[derive(Debug, Clone, Serialize, Deserialize, Builder)]
pub struct BacktestBaseline {
    /// Sharpe ratio from the original backtest.
    sharpe_ratio: f64,
    /// Maximum drawdown from the original backtest.
    max_drawdown: Decimal,
    /// Total `PnL` from the original backtest.
    pnl: Decimal,
}

impl BacktestBaseline {
    /// Returns the baseline Sharpe ratio.
    pub const fn sharpe_ratio(&self) -> f64 {
        self.sharpe_ratio
    }

    /// Returns the baseline maximum drawdown.
    pub const fn max_drawdown(&self) -> Decimal {
        self.max_drawdown
    }

    /// Returns the baseline `PnL`.
    pub const fn pnl(&self) -> Decimal {
        self.pnl
    }
}

/// Evaluates paper trading results against thresholds and the original
/// backtest baseline to decide whether a promoted strategy should be
/// confirmed or sent back for retraining.
///
/// Closing the full loop: research -> paper -> feedback -> research.
#[derive(Builder)]
pub struct RetrainChecker {
    /// The event bus used to publish retrain/confirmed events.
    event_bus: Arc<EventBus>,
    /// Absolute Sharpe floor — retrain if paper Sharpe drops below this.
    #[builder(default = 0.5)]
    sharpe_threshold: f64,
    /// Absolute drawdown ceiling — retrain if paper drawdown exceeds this.
    #[builder(default = Decimal::new(20, 2))]
    max_drawdown_threshold: Decimal,
    /// Fraction of original backtest Sharpe — retrain if paper Sharpe falls
    /// below this fraction of the baseline (e.g. 0.5 = 50% degradation).
    #[builder(default = 0.5)]
    degradation_factor: f64,
}

impl RetrainChecker {
    /// Evaluate paper trading results for the given experiment and decide
    /// whether to confirm the strategy or request retraining.
    ///
    /// Publishes either:
    /// - `feedback.strategy.confirmed` when the strategy is healthy
    /// - `feedback.research.retrain.requested` when degradation is detected
    pub fn evaluate(
        &self,
        experiment_id: Uuid,
        paper_metrics: &PaperMetrics,
        original_backtest: &BacktestBaseline,
    ) -> Result<RetrainDecision> {
        let reasons = self.check_degradation(paper_metrics, original_backtest);

        let decision = if reasons.is_empty() {
            let event = Event::builder()
                .event_type("feedback.strategy.confirmed")
                .source("feedback-bridge")
                .correlation_id(experiment_id.to_string())
                .payload(serde_json::json!({
                    "experiment_id": experiment_id.to_string(),
                    "sharpe_ratio": paper_metrics.sharpe_ratio(),
                    "max_drawdown": paper_metrics.max_drawdown().to_string(),
                }))
                .build();

            self.event_bus.publish(&event).context(EventBusSnafu)?;

            RetrainDecision::Confirmed
        } else {
            let combined_reason = reasons.join("; ");

            let event = Event::builder()
                .event_type("feedback.research.retrain.requested")
                .source("feedback-bridge")
                .correlation_id(experiment_id.to_string())
                .payload(serde_json::json!({
                    "experiment_id": experiment_id.to_string(),
                    "reason": combined_reason,
                    "sharpe_ratio": paper_metrics.sharpe_ratio(),
                    "max_drawdown": paper_metrics.max_drawdown().to_string(),
                }))
                .build();

            self.event_bus.publish(&event).context(EventBusSnafu)?;

            RetrainDecision::RetrainRequested {
                reason: combined_reason,
            }
        };

        Ok(decision)
    }

    /// Check all degradation conditions and return a list of reasons (empty
    /// if the strategy is healthy).
    fn check_degradation(
        &self,
        paper: &PaperMetrics,
        baseline: &BacktestBaseline,
    ) -> Vec<String> {
        let mut reasons = Vec::new();

        if paper.sharpe_ratio() < self.sharpe_threshold {
            reasons.push(format!(
                "Sharpe {:.2} below absolute threshold {:.2}",
                paper.sharpe_ratio(),
                self.sharpe_threshold,
            ));
        }

        if paper.max_drawdown() > self.max_drawdown_threshold {
            reasons.push(format!(
                "drawdown {} exceeds threshold {}",
                paper.max_drawdown(),
                self.max_drawdown_threshold,
            ));
        }

        let sharpe_floor = baseline.sharpe_ratio() * self.degradation_factor;
        if paper.sharpe_ratio() < sharpe_floor {
            reasons.push(format!(
                "Sharpe {:.2} degraded below {:.0}% of baseline {:.2}",
                paper.sharpe_ratio(),
                self.degradation_factor * 100.0,
                baseline.sharpe_ratio(),
            ));
        }

        reasons
    }
}

#[cfg(test)]
mod tests {
    use rust_decimal_macros::dec;

    use super::*;

    fn setup() -> (Arc<EventBus>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let bus = Arc::new(EventBus::open(dir.path()).unwrap());
        (bus, dir)
    }

    fn good_paper_metrics() -> PaperMetrics {
        PaperMetrics::builder()
            .sharpe_ratio(2.0)
            .max_drawdown(dec!(0.05))
            .pnl(dec!(1000))
            .win_rate(0.65)
            .trade_count(50)
            .build()
    }

    fn baseline() -> BacktestBaseline {
        BacktestBaseline::builder()
            .sharpe_ratio(2.5)
            .max_drawdown(dec!(0.03))
            .pnl(dec!(1200))
            .build()
    }

    #[test]
    fn confirms_healthy_strategy() {
        let (bus, _dir) = setup();
        let checker = RetrainChecker::builder()
            .event_bus(Arc::clone(&bus))
            .build();

        let decision = checker
            .evaluate(Uuid::new_v4(), &good_paper_metrics(), &baseline())
            .unwrap();

        assert_eq!(decision, RetrainDecision::Confirmed);

        let events = bus.store().read_topic("feedback", 0, 10).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type(), "feedback.strategy.confirmed");
    }

    #[test]
    fn retrain_on_low_sharpe() {
        let (bus, _dir) = setup();
        let checker = RetrainChecker::builder()
            .event_bus(Arc::clone(&bus))
            .build();

        let poor = PaperMetrics::builder()
            .sharpe_ratio(0.3)
            .max_drawdown(dec!(0.05))
            .pnl(dec!(100))
            .win_rate(0.45)
            .trade_count(50)
            .build();

        let decision = checker
            .evaluate(Uuid::new_v4(), &poor, &baseline())
            .unwrap();

        assert!(matches!(decision, RetrainDecision::RetrainRequested { .. }));

        let events = bus.store().read_topic("feedback", 0, 10).unwrap();
        assert_eq!(
            events[0].event_type(),
            "feedback.research.retrain.requested"
        );
    }

    #[test]
    fn retrain_on_excessive_drawdown() {
        let (bus, _dir) = setup();
        let checker = RetrainChecker::builder()
            .event_bus(Arc::clone(&bus))
            .build();

        let poor = PaperMetrics::builder()
            .sharpe_ratio(2.0)
            .max_drawdown(dec!(0.30))
            .pnl(dec!(500))
            .win_rate(0.55)
            .trade_count(50)
            .build();

        let decision = checker
            .evaluate(Uuid::new_v4(), &poor, &baseline())
            .unwrap();

        assert!(matches!(decision, RetrainDecision::RetrainRequested { .. }));
        if let RetrainDecision::RetrainRequested { reason } = &decision {
            assert!(reason.contains("drawdown"));
        }
    }

    #[test]
    fn retrain_on_sharpe_degradation_relative_to_baseline() {
        let (bus, _dir) = setup();
        let checker = RetrainChecker::builder()
            .event_bus(Arc::clone(&bus))
            .build();

        // Sharpe 1.0 is above absolute threshold (0.5) but below 50% of
        // baseline (2.5 * 0.5 = 1.25), so degradation triggers retrain.
        let degraded = PaperMetrics::builder()
            .sharpe_ratio(1.0)
            .max_drawdown(dec!(0.05))
            .pnl(dec!(300))
            .win_rate(0.50)
            .trade_count(50)
            .build();

        let decision = checker
            .evaluate(Uuid::new_v4(), &degraded, &baseline())
            .unwrap();

        assert!(matches!(decision, RetrainDecision::RetrainRequested { .. }));
        if let RetrainDecision::RetrainRequested { reason } = &decision {
            assert!(reason.contains("degraded"));
        }
    }

    #[test]
    fn custom_thresholds_are_respected() {
        let (bus, _dir) = setup();
        let checker = RetrainChecker::builder()
            .event_bus(Arc::clone(&bus))
            .sharpe_threshold(1.0)
            .max_drawdown_threshold(dec!(0.10))
            .degradation_factor(0.8)
            .build();

        // Sharpe 0.9 is below custom threshold of 1.0
        let poor = PaperMetrics::builder()
            .sharpe_ratio(0.9)
            .max_drawdown(dec!(0.05))
            .pnl(dec!(200))
            .win_rate(0.50)
            .trade_count(30)
            .build();

        let decision = checker
            .evaluate(Uuid::new_v4(), &poor, &baseline())
            .unwrap();

        assert!(matches!(decision, RetrainDecision::RetrainRequested { .. }));
    }
}
