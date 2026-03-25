//! Integration test for the backtest performance pipeline.
//!
//! Validates the full flow: write `.rara` files -> `DataCache` -> `BacktestPool` -> results.

use std::sync::Arc;

use async_trait::async_trait;
use rust_decimal::Decimal;

use rara_domain::research::BacktestResult;
use rara_market_data::cache::{DataCache, DataType};
use rara_market_data::ingester::{write_candles_to_file, RawCandle};
use rara_research::backtest_pool::{BacktestPool, BacktestTask};
use rara_research::backtester::{BacktestError, Backtester};

/// A backtester that returns fixed metrics for testing.
struct FixedBacktester;

#[async_trait]
impl Backtester for FixedBacktester {
    async fn run(
        &self,
        _strategy_code: &str,
        _contract_id: &str,
    ) -> Result<BacktestResult, BacktestError> {
        Ok(BacktestResult::builder()
            .pnl(Decimal::new(500, 0))
            .sharpe_ratio(1.8)
            .max_drawdown(Decimal::new(3, 2))
            .win_rate(0.65)
            .trade_count(100)
            .build())
    }
}

#[allow(clippy::cast_precision_loss)]
fn sample_candles(count: u32, base_ts: i64) -> Vec<RawCandle> {
    (0..count)
        .map(|i| RawCandle {
            timestamp_ns: base_ts + i64::from(i) * 60_000_000_000, // 1min apart
            open: 42000.0 + f64::from(i),
            high: 43000.0 + f64::from(i),
            low: 41000.0 + f64::from(i),
            close: 42500.0 + f64::from(i),
            volume: 100.0 + f64::from(i),
            trade_count: 100,
        })
        .collect()
}

#[tokio::test]
async fn full_pipeline_write_cache_backtest() {
    let dir = tempfile::tempdir().expect("tempdir");

    // 1. Write sample .rara files
    let written = write_candles_to_file(
        dir.path(),
        "binance-btc_usdt",
        "candles_1m",
        "2026-03-24",
        &sample_candles(100, 1_000_000_000),
    )
    .expect("write day 1");
    assert_eq!(written, 100);

    let written = write_candles_to_file(
        dir.path(),
        "binance-btc_usdt",
        "candles_1m",
        "2026-03-25",
        &sample_candles(50, 2_000_000_000),
    )
    .expect("write day 2");
    assert_eq!(written, 50);

    // 2. Load via DataCache
    let cache = DataCache::new(dir.path().to_path_buf(), 1024 * 1024 * 1024);
    let slices = cache
        .load_range(
            "binance-btc_usdt",
            DataType::Candle1m,
            "2026-03-24",
            "2026-03-25",
        )
        .expect("load_range");
    assert_eq!(slices.len(), 2);
    assert_eq!(slices[0].candle_count().expect("count day 1"), 100);
    assert_eq!(slices[1].candle_count().expect("count day 2"), 50);

    // 3. Verify cache hit returns the same Arc
    let slices2 = cache
        .load_range(
            "binance-btc_usdt",
            DataType::Candle1m,
            "2026-03-24",
            "2026-03-25",
        )
        .expect("load_range cached");
    assert!(Arc::ptr_eq(&slices[0], &slices2[0]));

    // 4. Run parallel backtests via BacktestPool
    let backtester = Arc::new(FixedBacktester);
    let pool = BacktestPool::with_concurrency(backtester, 4);

    let tasks: Vec<BacktestTask> = (0..6)
        .map(|i| BacktestTask {
            id: format!("strategy-{i}"),
            strategy_code: format!("code_{i}"),
            contract_id: "binance-btc_usdt".to_string(),
        })
        .collect();

    let results = pool.run_batch(tasks).await;
    assert_eq!(results.len(), 6);
    assert!(results.iter().all(Result::is_ok));

    // Verify result values
    let first = results[0].as_ref().expect("first result");
    assert_eq!(first.pnl, Decimal::new(500, 0));
    assert_eq!(first.trade_count, 100);
}
