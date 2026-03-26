//! Signal generation loop — connects candle stream to strategy execution.
//!
//! Receives [`AggregatedCandle`]s from a broadcast channel, feeds them to every
//! loaded WASM strategy, converts resulting [`Signal`]s into [`TradingCommit`]s,
//! and executes them through the [`TradingEngine`].

use std::sync::Arc;

use rust_decimal::Decimal;
use snafu::Snafu;
use tokio::sync::broadcast;

use rara_domain::trading::{ActionType, OrderType, Side, StagedAction, TradingCommit};
use rara_market_data::stream::aggregator::AggregatedCandle;
use rara_research::strategy_executor::StrategyHandle;
use rara_strategy_api::{Candle, Signal};

use crate::engine::TradingEngine;

/// Errors from the signal processing loop.
#[derive(Debug, Snafu)]
#[snafu(visibility(pub))]
pub enum SignalLoopError {
    /// Strategy signal generation failed.
    #[snafu(display("strategy '{name}' signal error: {source}"))]
    StrategySignal {
        /// Strategy name that failed.
        name: String,
        /// Underlying executor error.
        source: rara_research::strategy_executor::ExecutorError,
    },

    /// Trade execution failed.
    #[snafu(display("trade execution failed: {source}"))]
    Execution {
        /// Underlying engine error.
        source: crate::engine::EngineError,
    },
}

/// Result type for signal loop operations.
pub type Result<T> = std::result::Result<T, SignalLoopError>;

/// A loaded strategy ready for live signal generation.
///
/// Wraps a [`StrategyHandle`] with the metadata needed to construct
/// [`TradingCommit`]s from its signals.
pub struct LoadedStrategy {
    /// Strategy name (from WASM metadata).
    pub name: String,
    /// Strategy version (from WASM metadata).
    pub version: u32,
    /// Contract this strategy trades (e.g., "BTCUSDT").
    pub contract_id: String,
    /// Position size per signal.
    pub position_size: Decimal,
    /// The WASM strategy handle for signal generation.
    pub handle: Box<dyn StrategyHandle>,
}

/// Convert a strategy [`Signal`] into a list of [`StagedAction`]s.
///
/// Returns an empty vec for [`Signal::Hold`], which means "do nothing".
fn signal_to_actions(signal: &Signal, strategy: &LoadedStrategy) -> Vec<StagedAction> {
    match signal {
        Signal::Entry { side, .. } => {
            let domain_side = match side {
                rara_strategy_api::Side::Long => Side::Buy,
                rara_strategy_api::Side::Short => Side::Sell,
            };
            vec![StagedAction::builder()
                .action_type(ActionType::PlaceOrder)
                .contract_id(&strategy.contract_id)
                .side(domain_side)
                .quantity(strategy.position_size)
                .order_type(OrderType::Market)
                .build()]
        }
        Signal::Exit => {
            vec![StagedAction::builder()
                .action_type(ActionType::ClosePosition)
                .contract_id(&strategy.contract_id)
                .side(Side::Sell)
                .quantity(strategy.position_size)
                .order_type(OrderType::Market)
                .build()]
        }
        Signal::Hold => vec![],
    }
}

/// Convert an [`AggregatedCandle`] into the strategy-api [`Candle`] format.
const fn to_api_candle(candle: &AggregatedCandle) -> Candle {
    Candle {
        timestamp: candle.ts.timestamp(),
        open: candle.open,
        high: candle.high,
        low: candle.low,
        close: candle.close,
        volume: candle.volume,
    }
}

/// Run the signal generation loop.
///
/// Continuously receives candles from `candle_rx`, invokes `on_candles` on each
/// loaded strategy whose `contract_id` matches the candle symbol, and executes
/// any resulting trades through the engine. The loop exits when the broadcast
/// channel is closed.
pub async fn run_signal_loop(
    mut candle_rx: broadcast::Receiver<AggregatedCandle>,
    engine: Arc<TradingEngine>,
    mut strategies: Vec<LoadedStrategy>,
) {
    tracing::info!(
        strategy_count = strategies.len(),
        "signal loop started"
    );

    loop {
        match candle_rx.recv().await {
            Ok(candle) => {
                let api_candle = to_api_candle(&candle);
                process_candle(&candle, &api_candle, &mut strategies, &engine).await;
            }
            Err(broadcast::error::RecvError::Lagged(n)) => {
                tracing::warn!(skipped = n, "signal loop lagged, dropped candles");
            }
            Err(broadcast::error::RecvError::Closed) => {
                tracing::info!("candle channel closed, signal loop shutting down");
                break;
            }
        }
    }
}

/// Process a single candle against all matching strategies.
async fn process_candle(
    candle: &AggregatedCandle,
    api_candle: &Candle,
    strategies: &mut [LoadedStrategy],
    engine: &TradingEngine,
) {
    for strategy in strategies.iter_mut() {
        // Only feed candles to strategies trading this symbol
        if strategy.contract_id != candle.symbol {
            continue;
        }

        let signal = match strategy.handle.on_candles(std::slice::from_ref(api_candle)) {
            Ok(sig) => sig,
            Err(e) => {
                tracing::error!(
                    strategy = strategy.name,
                    error = %e,
                    "strategy signal error"
                );
                continue;
            }
        };

        let actions = signal_to_actions(&signal, strategy);
        if actions.is_empty() {
            continue;
        }

        tracing::info!(
            strategy = strategy.name,
            symbol = candle.symbol,
            signal = ?signal,
            action_count = actions.len(),
            "signal generated"
        );

        let commit = TradingCommit::builder()
            .message(format!("{} signal on {}", strategy.name, candle.symbol))
            .actions(actions)
            .strategy_id(&strategy.name)
            .strategy_version(strategy.version)
            .build();

        match engine.execute_commit(commit).await {
            Ok(results) => {
                for r in &results {
                    tracing::info!(
                        strategy = strategy.name,
                        order_id = r.order_id,
                        status = ?r.status,
                        "order executed"
                    );
                }
            }
            Err(e) => {
                tracing::error!(
                    strategy = strategy.name,
                    error = %e,
                    "trade execution failed"
                );
            }
        }
    }
}
