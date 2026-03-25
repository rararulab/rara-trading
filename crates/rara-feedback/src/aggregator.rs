//! Aggregates trading events from the event bus into strategy performance
//! metrics.

use std::sync::Arc;

use rust_decimal::Decimal;
use snafu::{ResultExt, Snafu};

use rara_domain::feedback::StrategyMetrics;
use rara_event_bus::bus::EventBus;
use rara_event_bus::store::StoreError;

/// Errors from metrics aggregation.
#[derive(Debug, Snafu)]
#[snafu(visibility(pub))]
pub enum AggregatorError {
    /// Failed to read events from the event bus.
    #[snafu(display("event bus error: {source}"))]
    EventBus {
        /// The underlying store error.
        source: StoreError,
    },
}

/// Alias for aggregator results.
pub type Result<T> = std::result::Result<T, AggregatorError>;

/// Aggregates trading events from the [`EventBus`] into
/// [`StrategyMetrics`] for a given strategy and time window.
pub struct MetricsAggregator {
    event_bus: Arc<EventBus>,
}

impl MetricsAggregator {
    /// Create a new aggregator backed by the given event bus.
    pub const fn new(event_bus: Arc<EventBus>) -> Self {
        Self { event_bus }
    }

    /// Aggregate trading events for `strategy_id` within the time window
    /// into [`StrategyMetrics`].
    ///
    /// Reads all `trading.*` events, filters by strategy and timestamp,
    /// then computes `PnL`, trade count, win rate, max drawdown, and Sharpe
    /// ratio. Returns zero metrics when no matching trades are found.
    pub fn aggregate(
        &self,
        strategy_id: &str,
        window_start: jiff::Timestamp,
        window_end: jiff::Timestamp,
    ) -> Result<StrategyMetrics> {
        let events = self
            .event_bus
            .store()
            .read_topic("trading", 0, usize::MAX)
            .context(EventBusSnafu)?;

        // Filter to filled orders for this strategy within the window
        let fills: Vec<_> = events
            .iter()
            .filter(|e| {
                e.event_type == "trading.order.filled"
                    && e.strategy_id.as_deref() == Some(strategy_id)
                    && e.timestamp >= window_start
                    && e.timestamp <= window_end
            })
            .collect();

        if fills.is_empty() {
            return Ok(StrategyMetrics::builder()
                .pnl(Decimal::ZERO)
                .sharpe_ratio(0.0)
                .max_drawdown(Decimal::ZERO)
                .win_rate(0.0)
                .trade_count(0)
                .build());
        }

        // Extract PnL per trade from payload (if present), otherwise assume zero
        let pnls: Vec<Decimal> = fills
            .iter()
            .map(|e| {
                e.payload
                    .get("realized_pnl")
                    .and_then(|v| v.as_str())
                    .and_then(|s| s.parse::<Decimal>().ok())
                    .unwrap_or(Decimal::ZERO)
            })
            .collect();

        let trade_count = u32::try_from(pnls.len()).unwrap_or(u32::MAX);
        let total_pnl: Decimal = pnls.iter().copied().sum();
        let wins = pnls.iter().filter(|p| **p > Decimal::ZERO).count();
        #[allow(clippy::cast_precision_loss)]
        let win_rate = if pnls.is_empty() {
            0.0
        } else {
            wins as f64 / pnls.len() as f64
        };

        // Max drawdown: largest peak-to-trough decline in cumulative PnL
        let max_drawdown = compute_max_drawdown(&pnls);

        // Sharpe ratio: mean(pnl) / std(pnl), annualized is out of scope for MVP
        let sharpe_ratio = compute_sharpe(&pnls);

        Ok(StrategyMetrics::builder()
            .pnl(total_pnl)
            .sharpe_ratio(sharpe_ratio)
            .max_drawdown(max_drawdown)
            .win_rate(win_rate)
            .trade_count(trade_count)
            .build())
    }
}

/// Compute the maximum drawdown from a series of per-trade `PnL` values.
fn compute_max_drawdown(pnls: &[Decimal]) -> Decimal {
    let mut peak = Decimal::ZERO;
    let mut max_dd = Decimal::ZERO;
    let mut cumulative = Decimal::ZERO;

    for pnl in pnls {
        cumulative += pnl;
        if cumulative > peak {
            peak = cumulative;
        }
        let drawdown = peak - cumulative;
        if drawdown > max_dd {
            max_dd = drawdown;
        }
    }

    max_dd
}

/// Compute a simplified Sharpe ratio (mean / stddev) from per-trade `PnL`.
fn compute_sharpe(pnls: &[Decimal]) -> f64 {
    if pnls.is_empty() {
        return 0.0;
    }

    let values: Vec<f64> = pnls
        .iter()
        .map(|d| d.to_string().parse::<f64>().unwrap_or(0.0))
        .collect();

    #[allow(clippy::cast_precision_loss)]
    let n = values.len() as f64;
    let mean = values.iter().sum::<f64>() / n;
    let variance = values.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / n;
    let stddev = variance.sqrt();

    if stddev < f64::EPSILON {
        0.0
    } else {
        mean / stddev
    }
}
