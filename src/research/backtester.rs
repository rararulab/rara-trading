//! Backtester trait and mock implementation for strategy evaluation.

use async_trait::async_trait;
use rust_decimal::Decimal;
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

/// A mock backtester that returns pre-configured results for testing.
pub struct MockBacktester {
    results: std::sync::Mutex<Vec<BacktestResult>>,
}

impl MockBacktester {
    /// Create a new mock backtester with a queue of results.
    pub const fn new(results: Vec<BacktestResult>) -> Self {
        Self {
            results: std::sync::Mutex::new(results),
        }
    }
}

#[async_trait]
impl Backtester for MockBacktester {
    async fn run(
        &self,
        _strategy_code: &str,
        _contract_id: &str,
    ) -> Result<BacktestResult, BacktestError> {
        let mut queue = self.results.lock().expect("mock lock poisoned");
        if queue.is_empty() {
            // Default passing result
            Ok(BacktestResult::builder()
                .pnl(Decimal::new(1000, 0))
                .sharpe_ratio(1.5)
                .max_drawdown(Decimal::new(5, 2))
                .win_rate(0.6)
                .trade_count(100)
                .build())
        } else {
            Ok(queue.remove(0))
        }
    }
}
