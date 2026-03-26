//! Background candle persistence to `TimescaleDB`.
//!
//! Consumes aggregated candles from a broadcast channel and writes them
//! to the database without blocking the main trading flow.

use tokio::sync::broadcast;

use super::aggregator::AggregatedCandle;
use crate::store::candle::CandleRow;
use crate::store::MarketStore;

/// Run the candle persister loop, consuming candles and writing to the store.
///
/// Converts each `AggregatedCandle` into a `CandleRow` and upserts it via
/// `MarketStore::insert_candles`. Runs until the broadcast sender is dropped.
pub async fn run_candle_persister(
    mut receiver: broadcast::Receiver<AggregatedCandle>,
    store: MarketStore,
    instrument_prefix: &str,
) {
    tracing::info!("candle persister started");

    loop {
        match receiver.recv().await {
            Ok(candle) => {
                let instrument_id = format!("{}-{}", instrument_prefix, candle.symbol);
                let row = CandleRow {
                    ts: candle.ts,
                    instrument_id: instrument_id.clone(),
                    interval: candle.interval.clone(),
                    open: candle.open,
                    high: candle.high,
                    low: candle.low,
                    close: candle.close,
                    volume: candle.volume,
                    trade_count: candle.trade_count,
                };

                match store.insert_candles(&[row]).await {
                    Ok(count) => {
                        tracing::debug!(
                            instrument_id,
                            interval = candle.interval,
                            inserted = count,
                            "persisted candle"
                        );
                    }
                    Err(e) => {
                        tracing::error!(
                            error = %e,
                            instrument_id,
                            interval = candle.interval,
                            "failed to persist candle"
                        );
                    }
                }
            }
            Err(broadcast::error::RecvError::Lagged(n)) => {
                tracing::warn!(skipped = n, "candle persister lagged, skipped candles");
            }
            Err(broadcast::error::RecvError::Closed) => {
                tracing::info!("candle persister channel closed, shutting down");
                break;
            }
        }
    }
}

/// Spawn the candle persister as a background tokio task.
///
/// Returns the `JoinHandle` so callers can optionally await shutdown.
pub fn spawn_candle_persister(
    receiver: broadcast::Receiver<AggregatedCandle>,
    store: MarketStore,
    instrument_prefix: String,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        run_candle_persister(receiver, store, &instrument_prefix).await;
    })
}
