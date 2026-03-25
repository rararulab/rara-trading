//! Backtester trait for strategy evaluation.

use async_trait::async_trait;
use snafu::Snafu;

use rara_domain::research::BacktestResult;
use rara_domain::timeframe::Timeframe;

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

/// Trait for running backtests with compiled strategy artifacts.
#[async_trait]
pub trait Backtester: Send + Sync {
    /// Run a backtest with compiled strategy artifact, contract, and timeframe.
    async fn run(
        &self,
        strategy_artifact: &[u8],
        contract_id: &str,
        timeframe: Timeframe,
    ) -> Result<BacktestResult, BacktestError>;
}
