//! Backtester trait for strategy evaluation.

use std::sync::Arc;

use async_trait::async_trait;
use snafu::Snafu;

use rara_domain::research::BacktestResult;
use rara_domain::timeframe::Timeframe;
use rara_market_data::cache::MarketSlice;

/// Errors from backtesting operations.
#[derive(Debug, Snafu)]
#[snafu(visibility(pub))]
pub enum BacktestError {
    /// The backtest execution failed.
    #[snafu(display("backtest failed: {message}"))]
    ExecutionFailed {
        /// Description of the failure.
        message: String,
    },
}

/// Trait for running backtests with compiled WASM strategy bytes.
#[async_trait]
pub trait Backtester: Send + Sync {
    /// Run a backtest with compiled strategy bytes, contract, and timeframe.
    async fn run(
        &self,
        wasm_bytes: &[u8],
        contract_id: &str,
        timeframe: Timeframe,
    ) -> Result<BacktestResult, BacktestError>;

    /// Run a backtest with pre-loaded market data slices.
    ///
    /// Default implementation ignores the data and falls back to [`run`](Self::run).
    /// Implementors should override this to use the cached data directly,
    /// avoiding redundant disk I/O.
    async fn run_with_data(
        &self,
        wasm_bytes: &[u8],
        contract_id: &str,
        timeframe: Timeframe,
        _data: &[Arc<MarketSlice>],
    ) -> Result<BacktestResult, BacktestError> {
        self.run(wasm_bytes, contract_id, timeframe).await
    }
}
