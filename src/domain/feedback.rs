//! Feedback loop types — strategy evaluation and lifecycle decisions.

use bon::Builder;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Decision about a strategy's lifecycle progression.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FeedbackDecision {
    /// Advance the strategy to the next stage.
    Promote,
    /// Keep the strategy at its current stage.
    Hold,
    /// Move the strategy back a stage.
    Demote,
    /// Permanently retire the strategy.
    Retire,
}

/// Aggregated performance metrics for a strategy over a time window.
#[derive(Debug, Clone, Serialize, Deserialize, Builder)]
pub struct StrategyMetrics {
    /// Profit and loss.
    pnl: Decimal,
    /// Sharpe ratio.
    sharpe_ratio: f64,
    /// Maximum drawdown.
    max_drawdown: Decimal,
    /// Win rate as a fraction.
    win_rate: f64,
    /// Total number of trades.
    trade_count: u32,
}

impl StrategyMetrics {
    /// Returns the Sharpe ratio.
    pub const fn sharpe_ratio(&self) -> f64 {
        self.sharpe_ratio
    }

    /// Returns the maximum drawdown.
    pub const fn max_drawdown(&self) -> Decimal {
        self.max_drawdown
    }
}

/// A periodic evaluation report for a strategy.
#[derive(Debug, Clone, Serialize, Deserialize, Builder)]
pub struct StrategyReport {
    #[builder(into)]
    strategy_id: String,
    strategy_version: u32,
    window_start: jiff::Timestamp,
    window_end: jiff::Timestamp,
    metrics: StrategyMetrics,
    sentinel_events: Vec<Uuid>,
    decision: FeedbackDecision,
    #[builder(into)]
    reason: String,
    #[builder(default = jiff::Timestamp::now())]
    generated_at: jiff::Timestamp,
}

impl StrategyReport {
    /// Returns the lifecycle decision.
    pub const fn decision(&self) -> FeedbackDecision {
        self.decision
    }

    /// Returns the strategy identifier.
    pub fn strategy_id(&self) -> &str {
        &self.strategy_id
    }

    /// Returns `true` if the strategy should be retrained (i.e. retired).
    pub fn should_trigger_retrain(&self) -> bool {
        self.decision == FeedbackDecision::Retire
    }
}
