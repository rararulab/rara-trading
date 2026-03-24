//! Strategy evaluation logic — maps metrics + sentinel context to a lifecycle
//! decision.

use rust_decimal::Decimal;

use crate::domain::feedback::{FeedbackDecision, StrategyMetrics};

/// Evaluates a strategy's performance metrics and sentinel context to produce
/// a [`FeedbackDecision`] controlling the strategy's lifecycle.
pub struct StrategyEvaluator {
    /// Minimum Sharpe ratio required to promote a strategy.
    promote_threshold: f64,
    /// Maximum drawdown that triggers demotion/retirement.
    demote_drawdown: Decimal,
    /// Minimum number of trades before a decision can be made.
    min_trades: u32,
}

impl StrategyEvaluator {
    /// Create a new evaluator with the given thresholds.
    pub const fn new(promote_threshold: f64, demote_drawdown: Decimal, min_trades: u32) -> Self {
        Self {
            promote_threshold,
            demote_drawdown,
            min_trades,
        }
    }

    /// Evaluate a strategy and return a lifecycle decision with a reason.
    ///
    /// Decision priority:
    /// 1. Critical sentinel signal → Demote (safety first)
    /// 2. Insufficient trades → Hold (not enough data)
    /// 3. Excessive drawdown → Retire (trigger retrain)
    /// 4. Strong Sharpe + positive win rate → Promote
    /// 5. Otherwise → Hold
    pub fn evaluate(
        &self,
        metrics: &StrategyMetrics,
        has_critical_sentinel: bool,
    ) -> (FeedbackDecision, String) {
        if has_critical_sentinel {
            return (
                FeedbackDecision::Demote,
                "critical sentinel signal detected — safety demotion".to_owned(),
            );
        }

        if metrics.trade_count() < self.min_trades {
            return (
                FeedbackDecision::Hold,
                format!(
                    "insufficient trades: {} < {} minimum",
                    metrics.trade_count(),
                    self.min_trades
                ),
            );
        }

        if metrics.max_drawdown() > self.demote_drawdown {
            return (
                FeedbackDecision::Retire,
                format!(
                    "excessive drawdown: {} > {} threshold — requesting retrain",
                    metrics.max_drawdown(),
                    self.demote_drawdown
                ),
            );
        }

        if metrics.sharpe_ratio() >= self.promote_threshold && metrics.win_rate() > 0.5 {
            return (
                FeedbackDecision::Promote,
                format!(
                    "strong performance: sharpe={:.2}, win_rate={:.2}",
                    metrics.sharpe_ratio(),
                    metrics.win_rate()
                ),
            );
        }

        (
            FeedbackDecision::Hold,
            format!(
                "metrics within tolerance: sharpe={:.2}, win_rate={:.2}, drawdown={}",
                metrics.sharpe_ratio(),
                metrics.win_rate(),
                metrics.max_drawdown()
            ),
        )
    }
}

#[cfg(test)]
mod tests {
    use rust_decimal_macros::dec;

    use super::*;

    fn evaluator() -> StrategyEvaluator {
        StrategyEvaluator::new(1.5, dec!(0.20), 10)
    }

    #[test]
    fn critical_sentinel_forces_demote() {
        let metrics = StrategyMetrics::builder()
            .pnl(dec!(5000))
            .sharpe_ratio(3.0)
            .max_drawdown(dec!(0.01))
            .win_rate(0.9)
            .trade_count(100)
            .build();

        let (decision, _) = evaluator().evaluate(&metrics, true);
        assert_eq!(decision, FeedbackDecision::Demote);
    }

    #[test]
    fn insufficient_trades_holds() {
        let metrics = StrategyMetrics::builder()
            .pnl(dec!(100))
            .sharpe_ratio(2.0)
            .max_drawdown(dec!(0.01))
            .win_rate(0.8)
            .trade_count(5)
            .build();

        let (decision, _) = evaluator().evaluate(&metrics, false);
        assert_eq!(decision, FeedbackDecision::Hold);
    }

    #[test]
    fn excessive_drawdown_retires() {
        let metrics = StrategyMetrics::builder()
            .pnl(dec!(-2000))
            .sharpe_ratio(0.5)
            .max_drawdown(dec!(0.30))
            .win_rate(0.3)
            .trade_count(50)
            .build();

        let (decision, _) = evaluator().evaluate(&metrics, false);
        assert_eq!(decision, FeedbackDecision::Retire);
    }

    #[test]
    fn strong_performance_promotes() {
        let metrics = StrategyMetrics::builder()
            .pnl(dec!(8000))
            .sharpe_ratio(2.5)
            .max_drawdown(dec!(0.05))
            .win_rate(0.65)
            .trade_count(100)
            .build();

        let (decision, _) = evaluator().evaluate(&metrics, false);
        assert_eq!(decision, FeedbackDecision::Promote);
    }

    #[test]
    fn mediocre_performance_holds() {
        let metrics = StrategyMetrics::builder()
            .pnl(dec!(500))
            .sharpe_ratio(1.0)
            .max_drawdown(dec!(0.10))
            .win_rate(0.45)
            .trade_count(50)
            .build();

        let (decision, _) = evaluator().evaluate(&metrics, false);
        assert_eq!(decision, FeedbackDecision::Hold);
    }
}
