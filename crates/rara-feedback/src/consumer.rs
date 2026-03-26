//! Event bus consumer for trading fill events.
//!
//! Subscribes to the `trading` topic, filters [`EventType::TradingOrderFilled`]
//! events, and maintains per-strategy running metric accumulators with
//! consumer-offset persistence so restarts don't reprocess old events.

use std::collections::HashMap;
use std::sync::Arc;

use rust_decimal::Decimal;
use snafu::{ResultExt, Snafu};

use rara_domain::event::{Event, EventType};
use rara_domain::feedback::StrategyMetrics;
use rara_event_bus::bus::EventBus;
use rara_event_bus::store::StoreError;

/// Errors from the feedback consumer.
#[derive(Debug, Snafu)]
#[snafu(visibility(pub))]
pub enum ConsumerError {
    /// Event bus read/write error.
    #[snafu(display("event bus error: {source}"))]
    EventBus {
        /// The underlying store error.
        source: StoreError,
    },
}

/// Result type for consumer operations.
pub type Result<T> = std::result::Result<T, ConsumerError>;

/// Consumer group name for offset tracking.
const CONSUMER_GROUP: &str = "feedback";
/// Topic to consume.
const TOPIC: &str = "trading";
/// Maximum events to read per poll batch.
const BATCH_SIZE: usize = 100;

/// Per-strategy running metrics accumulator.
///
/// Tracks trade counts, cumulative `PnL`, peak equity, max drawdown, and
/// individual returns for Sharpe ratio computation. Updated incrementally
/// as fill events arrive.
#[derive(Debug, Clone)]
pub struct StrategyAccumulator {
    /// Strategy identifier.
    pub strategy_id: String,
    /// Total number of filled trades.
    pub trade_count: u32,
    /// Number of winning trades (realized `PnL` > 0).
    pub winning_trades: u32,
    /// Number of losing trades (realized `PnL` <= 0).
    pub losing_trades: u32,
    /// Running total realized `PnL`.
    pub total_pnl: Decimal,
    /// Peak cumulative equity for drawdown calculation.
    pub peak_equity: Decimal,
    /// Maximum drawdown observed so far.
    pub max_drawdown: Decimal,
    /// Individual trade returns for Sharpe calculation.
    pub returns: Vec<f64>,
}

impl StrategyAccumulator {
    /// Create a new accumulator for the given strategy.
    pub const fn new(strategy_id: String) -> Self {
        Self {
            strategy_id,
            trade_count: 0,
            winning_trades: 0,
            losing_trades: 0,
            total_pnl: Decimal::ZERO,
            peak_equity: Decimal::ZERO,
            max_drawdown: Decimal::ZERO,
            returns: Vec::new(),
        }
    }

    /// Record a single trade fill with the given realized `PnL`.
    ///
    /// Updates trade counts, cumulative `PnL`, peak equity, max drawdown,
    /// and stores the return value for Sharpe ratio computation.
    pub fn record_fill(&mut self, pnl: Decimal) {
        self.trade_count += 1;
        self.total_pnl += pnl;

        if pnl > Decimal::ZERO {
            self.winning_trades += 1;
        } else {
            self.losing_trades += 1;
        }

        // Update peak equity and max drawdown
        if self.total_pnl > self.peak_equity {
            self.peak_equity = self.total_pnl;
        }
        let drawdown = self.peak_equity - self.total_pnl;
        if drawdown > self.max_drawdown {
            self.max_drawdown = drawdown;
        }

        // Store return for Sharpe calculation
        self.returns
            .push(pnl.to_string().parse::<f64>().unwrap_or(0.0));
    }

    /// Compute the Sharpe ratio from accumulated returns.
    ///
    /// Uses sample standard deviation (n-1 denominator). Returns 0.0 when
    /// fewer than 2 trades have been recorded or when all returns are identical.
    pub fn sharpe_ratio(&self) -> f64 {
        if self.returns.len() < 2 {
            return 0.0;
        }

        #[allow(clippy::cast_precision_loss)]
        let n = self.returns.len() as f64;
        let mean = self.returns.iter().sum::<f64>() / n;
        let variance = self.returns.iter().map(|r| (r - mean).powi(2)).sum::<f64>() / (n - 1.0);
        let std_dev = variance.sqrt();

        if std_dev < f64::EPSILON {
            return 0.0;
        }

        mean / std_dev
    }

    /// Convert the current accumulator state to [`StrategyMetrics`].
    pub fn to_metrics(&self) -> StrategyMetrics {
        StrategyMetrics::builder()
            .pnl(self.total_pnl)
            .sharpe_ratio(self.sharpe_ratio())
            .max_drawdown(self.max_drawdown)
            .win_rate(if self.trade_count > 0 {
                f64::from(self.winning_trades) / f64::from(self.trade_count)
            } else {
                0.0
            })
            .trade_count(self.trade_count)
            .build()
    }
}

/// Incremental event bus consumer that reads trading fill events and
/// maintains per-strategy metric accumulators.
///
/// Unlike [`MetricsAggregator`](crate::aggregator::MetricsAggregator) which
/// scans all events on each call, this consumer tracks its read offset and
/// only processes new events on each poll.
pub struct FeedbackConsumer {
    /// Event bus to consume from.
    event_bus: Arc<EventBus>,
    /// Per-strategy running accumulators.
    accumulators: HashMap<String, StrategyAccumulator>,
}

impl FeedbackConsumer {
    /// Create a new feedback consumer backed by the given event bus.
    pub fn new(event_bus: Arc<EventBus>) -> Self {
        Self {
            event_bus,
            accumulators: HashMap::new(),
        }
    }

    /// Get current metrics for all tracked strategies.
    pub fn all_metrics(&self) -> Vec<StrategyMetrics> {
        self.accumulators.values().map(StrategyAccumulator::to_metrics).collect()
    }

    /// Get current metrics for all tracked strategies, paired with their IDs.
    pub fn all_metrics_with_ids(&self) -> Vec<(String, StrategyMetrics)> {
        self.accumulators
            .iter()
            .map(|(id, acc)| (id.clone(), acc.to_metrics()))
            .collect()
    }

    /// Get metrics for a specific strategy, if any fills have been recorded.
    pub fn strategy_metrics(&self, strategy_id: &str) -> Option<StrategyMetrics> {
        self.accumulators.get(strategy_id).map(StrategyAccumulator::to_metrics)
    }

    /// Poll for new trading events and update accumulators.
    ///
    /// Reads up to [`BATCH_SIZE`] events from the stored offset, filters for
    /// `TradingOrderFilled`, records fills in the appropriate accumulator,
    /// and advances the consumer offset. Returns the number of fill events
    /// processed.
    pub fn poll(&mut self) -> Result<usize> {
        let offset = self
            .event_bus
            .store()
            .get_offset(CONSUMER_GROUP, TOPIC)
            .context(EventBusSnafu)?;

        let events = self
            .event_bus
            .store()
            .read_topic(TOPIC, offset, BATCH_SIZE)
            .context(EventBusSnafu)?;

        if events.is_empty() {
            return Ok(0);
        }

        let mut fills_processed = 0;
        for event in &events {
            if event.event_type == EventType::TradingOrderFilled {
                self.process_fill(event);
                fills_processed += 1;
            }
        }

        // Advance offset past all events we read (not just fills)
        // offset + events.len() works because read_topic returns sequential events
        #[allow(clippy::cast_possible_truncation)]
        let new_offset = offset + events.len() as u64;
        self.event_bus
            .store()
            .set_offset(CONSUMER_GROUP, TOPIC, new_offset)
            .context(EventBusSnafu)?;

        Ok(fills_processed)
    }

    /// Extract `PnL` from fill event payload and record it.
    fn process_fill(&mut self, event: &Event) {
        let strategy_id = event
            .strategy_id
            .clone()
            .unwrap_or_else(|| "unknown".to_string());

        // Extract realized_pnl from payload — try string first (Decimal-safe), then number
        let pnl = event
            .payload
            .get("realized_pnl")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<Decimal>().ok())
            .or_else(|| {
                event
                    .payload
                    .get("realized_pnl")
                    .and_then(serde_json::Value::as_f64)
                    .and_then(|f| Decimal::try_from(f).ok())
            })
            .unwrap_or(Decimal::ZERO);

        let accumulator = self
            .accumulators
            .entry(strategy_id.clone())
            .or_insert_with(|| StrategyAccumulator::new(strategy_id));

        accumulator.record_fill(pnl);
    }

    /// Run the consumer loop, polling at the given interval.
    ///
    /// This method runs indefinitely, polling for new events and logging
    /// processing activity. Errors are logged and retried on the next tick.
    pub async fn run(&mut self, poll_interval: std::time::Duration) {
        tracing::info!("feedback consumer started");
        let mut interval = tokio::time::interval(poll_interval);

        loop {
            interval.tick().await;
            match self.poll() {
                Ok(n) if n > 0 => {
                    tracing::info!(fills_processed = n, "processed trading fill events");
                }
                Ok(_) => {}
                Err(e) => {
                    tracing::error!(error = %e, "feedback consumer poll error");
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use rust_decimal_macros::dec;
    use serde_json::json;

    use rara_domain::event::{Event, EventType};
    use rara_event_bus::bus::EventBus;

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

    fn publish_non_fill(bus: &EventBus) {
        let event = Event::builder()
            .event_type(EventType::TradingOrderSubmitted)
            .source("test")
            .correlation_id("test-corr")
            .payload(json!({}))
            .build();
        bus.publish(&event).unwrap();
    }

    // --- StrategyAccumulator tests ---

    #[test]
    fn accumulator_tracks_win_loss_counts() {
        let mut acc = StrategyAccumulator::new("strat-1".into());
        acc.record_fill(dec!(100));
        acc.record_fill(dec!(-50));
        acc.record_fill(dec!(200));
        acc.record_fill(dec!(0)); // zero counts as a loss

        assert_eq!(acc.trade_count, 4);
        assert_eq!(acc.winning_trades, 2);
        assert_eq!(acc.losing_trades, 2);
        assert_eq!(acc.total_pnl, dec!(250));
    }

    #[test]
    fn accumulator_win_rate_calculation() {
        let mut acc = StrategyAccumulator::new("strat-1".into());
        acc.record_fill(dec!(100));
        acc.record_fill(dec!(-50));
        acc.record_fill(dec!(200));
        acc.record_fill(dec!(50));

        let metrics = acc.to_metrics();
        // 3 wins out of 4 trades = 0.75
        assert!((metrics.win_rate - 0.75).abs() < f64::EPSILON);
    }

    #[test]
    fn accumulator_max_drawdown_tracks_peak_to_trough() {
        let mut acc = StrategyAccumulator::new("strat-1".into());
        // Equity curve: 100, 250, 150, 200, 50
        acc.record_fill(dec!(100)); // peak=100, dd=0
        acc.record_fill(dec!(150)); // peak=250, dd=0
        acc.record_fill(dec!(-100)); // peak=250, dd=100
        acc.record_fill(dec!(50)); // peak=250, dd=50
        acc.record_fill(dec!(-150)); // peak=250, dd=200

        assert_eq!(acc.max_drawdown, dec!(200));
        assert_eq!(acc.peak_equity, dec!(250));
    }

    #[test]
    fn accumulator_sharpe_ratio_known_values() {
        let mut acc = StrategyAccumulator::new("strat-1".into());
        // Returns: [10, 20, 30]
        // mean = 20, sample_var = 100, sample_std = 10
        // sharpe = 20 / 10 = 2.0
        acc.record_fill(dec!(10));
        acc.record_fill(dec!(20));
        acc.record_fill(dec!(30));

        let sharpe = acc.sharpe_ratio();
        assert!((sharpe - 2.0).abs() < 1e-10);
    }

    #[test]
    fn accumulator_sharpe_ratio_zero_with_single_trade() {
        let mut acc = StrategyAccumulator::new("strat-1".into());
        acc.record_fill(dec!(100));

        assert!(acc.sharpe_ratio().abs() < f64::EPSILON);
    }

    #[test]
    fn accumulator_sharpe_ratio_zero_with_identical_returns() {
        let mut acc = StrategyAccumulator::new("strat-1".into());
        acc.record_fill(dec!(50));
        acc.record_fill(dec!(50));
        acc.record_fill(dec!(50));

        assert!(acc.sharpe_ratio().abs() < f64::EPSILON);
    }

    #[test]
    fn accumulator_to_metrics_maps_fields_correctly() {
        let mut acc = StrategyAccumulator::new("strat-1".into());
        acc.record_fill(dec!(100));
        acc.record_fill(dec!(-30));
        acc.record_fill(dec!(50));

        let m = acc.to_metrics();
        assert_eq!(m.pnl, dec!(120));
        assert_eq!(m.trade_count, 3);
        assert_eq!(m.max_drawdown, dec!(30));
        // 2 wins / 3 trades
        assert!((m.win_rate - 2.0 / 3.0).abs() < 1e-10);
    }

    // --- FeedbackConsumer tests ---

    #[test]
    fn consumer_processes_fills_and_skips_non_fills() {
        let (bus, _dir) = setup();

        publish_non_fill(&bus);
        publish_fill(&bus, "strat-1", "100");
        publish_non_fill(&bus);
        publish_fill(&bus, "strat-1", "-50");

        let mut consumer = FeedbackConsumer::new(Arc::clone(&bus));
        let processed = consumer.poll().unwrap();

        assert_eq!(processed, 2);
        let metrics = consumer.strategy_metrics("strat-1").unwrap();
        assert_eq!(metrics.trade_count, 2);
        assert_eq!(metrics.pnl, dec!(50));
    }

    #[test]
    fn consumer_aggregates_across_multiple_strategies() {
        let (bus, _dir) = setup();

        publish_fill(&bus, "alpha", "100");
        publish_fill(&bus, "beta", "-50");
        publish_fill(&bus, "alpha", "200");

        let mut consumer = FeedbackConsumer::new(Arc::clone(&bus));
        consumer.poll().unwrap();

        let alpha = consumer.strategy_metrics("alpha").unwrap();
        assert_eq!(alpha.trade_count, 2);
        assert_eq!(alpha.pnl, dec!(300));

        let beta = consumer.strategy_metrics("beta").unwrap();
        assert_eq!(beta.trade_count, 1);
        assert_eq!(beta.pnl, dec!(-50));
    }

    #[test]
    fn consumer_offset_persists_across_polls() {
        let (bus, _dir) = setup();

        publish_fill(&bus, "strat-1", "100");
        publish_fill(&bus, "strat-1", "200");

        let mut consumer = FeedbackConsumer::new(Arc::clone(&bus));
        let first = consumer.poll().unwrap();
        assert_eq!(first, 2);

        // Publish more events
        publish_fill(&bus, "strat-1", "300");

        let second = consumer.poll().unwrap();
        assert_eq!(second, 1);

        // Total should reflect all 3 fills
        let metrics = consumer.strategy_metrics("strat-1").unwrap();
        assert_eq!(metrics.trade_count, 3);
        assert_eq!(metrics.pnl, dec!(600));
    }

    #[test]
    fn consumer_no_reprocessing_after_restart() {
        let (bus, _dir) = setup();

        publish_fill(&bus, "strat-1", "100");

        // First consumer processes
        let mut consumer1 = FeedbackConsumer::new(Arc::clone(&bus));
        consumer1.poll().unwrap();

        // Simulate restart with a new consumer instance (same bus = same offset store)
        let mut consumer2 = FeedbackConsumer::new(Arc::clone(&bus));
        let processed = consumer2.poll().unwrap();

        // Should not reprocess the old event
        assert_eq!(processed, 0);
        assert!(consumer2.strategy_metrics("strat-1").is_none());
    }

    #[test]
    fn consumer_all_metrics_returns_all_strategies() {
        let (bus, _dir) = setup();

        publish_fill(&bus, "alpha", "100");
        publish_fill(&bus, "beta", "200");
        publish_fill(&bus, "gamma", "300");

        let mut consumer = FeedbackConsumer::new(Arc::clone(&bus));
        consumer.poll().unwrap();

        let all = consumer.all_metrics();
        assert_eq!(all.len(), 3);
    }

    #[test]
    fn consumer_handles_missing_pnl_as_zero() {
        let (bus, _dir) = setup();

        // Publish a fill without realized_pnl in payload
        let event = Event::builder()
            .event_type(EventType::TradingOrderFilled)
            .source("test")
            .correlation_id("test-corr")
            .strategy_id("strat-1".to_owned())
            .payload(json!({"symbol": "BTC/USD"}))
            .build();
        bus.publish(&event).unwrap();

        let mut consumer = FeedbackConsumer::new(Arc::clone(&bus));
        consumer.poll().unwrap();

        let metrics = consumer.strategy_metrics("strat-1").unwrap();
        assert_eq!(metrics.pnl, dec!(0));
        assert_eq!(metrics.trade_count, 1);
    }

    #[test]
    fn consumer_handles_numeric_pnl_payload() {
        let (bus, _dir) = setup();

        // Publish a fill with numeric (not string) realized_pnl
        let event = Event::builder()
            .event_type(EventType::TradingOrderFilled)
            .source("test")
            .correlation_id("test-corr")
            .strategy_id("strat-1".to_owned())
            .payload(json!({"realized_pnl": 150.5}))
            .build();
        bus.publish(&event).unwrap();

        let mut consumer = FeedbackConsumer::new(Arc::clone(&bus));
        consumer.poll().unwrap();

        let metrics = consumer.strategy_metrics("strat-1").unwrap();
        assert_eq!(metrics.trade_count, 1);
        // f64 → Decimal conversion may have slight representation, just check positive
        assert!(metrics.pnl > dec!(0));
    }
}
