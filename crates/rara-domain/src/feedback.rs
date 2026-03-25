//! Feedback loop types — strategy evaluation and lifecycle decisions.

use bon::Builder;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use strum::{Display, EnumString};
use uuid::Uuid;

/// Decision about a strategy's lifecycle progression.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Display, EnumString)]
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
    pub pnl: Decimal,
    /// Sharpe ratio.
    pub sharpe_ratio: f64,
    /// Maximum drawdown.
    pub max_drawdown: Decimal,
    /// Win rate as a fraction.
    pub win_rate: f64,
    /// Total number of trades.
    pub trade_count: u32,
}

/// A periodic evaluation report for a strategy.
#[derive(Debug, Clone, Serialize, Deserialize, Builder)]
pub struct StrategyReport {
    #[builder(into)]
    pub strategy_id: String,
    pub strategy_version: u32,
    pub window_start: jiff::Timestamp,
    pub window_end: jiff::Timestamp,
    pub metrics: StrategyMetrics,
    pub sentinel_events: Vec<Uuid>,
    pub decision: FeedbackDecision,
    #[builder(into)]
    pub reason: String,
    #[builder(default = jiff::Timestamp::now())]
    pub generated_at: jiff::Timestamp,
}

impl StrategyReport {
    /// Returns `true` if the strategy should be retrained (i.e. retired).
    pub fn should_trigger_retrain(&self) -> bool {
        self.decision == FeedbackDecision::Retire
    }
}
