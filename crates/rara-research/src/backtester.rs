//! Backtester trait for strategy evaluation.

use async_trait::async_trait;
use snafu::Snafu;

use rara_domain::research::BacktestResult;
use rara_domain::timeframe::Timeframe;

use crate::strategy_executor::StrategyHandle;

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

/// Trait for running backtests with a loaded strategy handle.
#[async_trait]
pub trait Backtester: Send + Sync {
    /// Run a backtest with a loaded strategy handle, contract, and timeframe.
    async fn run(
        &self,
        handle: Box<dyn StrategyHandle>,
        contract_id: &str,
        timeframe: Timeframe,
    ) -> Result<BacktestResult, BacktestError>;
}
