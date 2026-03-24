//! Backtester trait for strategy evaluation.

use async_trait::async_trait;
use snafu::Snafu;

use crate::domain::research::BacktestResult;

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

/// Trait for running backtests against strategy code.
#[async_trait]
pub trait Backtester: Send + Sync {
    /// Run a backtest with the given strategy code and contract.
    async fn run(
        &self,
        strategy_code: &str,
        contract_id: &str,
    ) -> Result<BacktestResult, BacktestError>;
}
