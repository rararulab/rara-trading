//! Parallel backtest scheduler with semaphore-based concurrency.

use std::sync::Arc;

use snafu::Snafu;
use tokio::sync::Semaphore;

use crate::backtester::{BacktestError, Backtester};
use rara_domain::research::BacktestResult;
use rara_domain::timeframe::Timeframe;

/// Errors from pool operations.
#[derive(Debug, Snafu)]
#[snafu(visibility(pub))]
pub enum PoolError {
    /// A backtest task failed during execution.
    #[snafu(display("backtest '{task_id}' failed: {source}"))]
    TaskFailed {
        /// Identifier of the failed task.
        task_id: String,
        /// Underlying backtest error.
        source: BacktestError,
    },
}

/// A single backtest task to be executed.
#[derive(Debug, Clone)]
pub struct BacktestTask {
    /// Unique identifier for this task.
    pub id: String,
    /// Compiled WASM strategy bytes.
    pub strategy_artifact: Arc<Vec<u8>>,
    /// Contract to run the backtest against.
    pub contract_id: String,
    /// Target timeframe for candle aggregation.
    pub timeframe: Timeframe,
}

/// Parallel backtest scheduler that limits concurrency via a semaphore.
pub struct BacktestPool<B: Backtester> {
    /// Maximum number of concurrent backtests.
    concurrency: usize,
    /// Shared backtester implementation.
    backtester: Arc<B>,
}

impl<B: Backtester + 'static> BacktestPool<B> {
    /// Create a new pool with the given backtester and default concurrency
    /// (`num_cpus` - 1, minimum 1).
    pub fn new(backtester: Arc<B>) -> Self {
        let concurrency = num_cpus::get().saturating_sub(1).max(1);
        Self {
            concurrency,
            backtester,
        }
    }

    /// Create a new pool with explicit concurrency limit.
    pub fn with_concurrency(backtester: Arc<B>, concurrency: usize) -> Self {
        Self {
            concurrency: concurrency.max(1),
            backtester,
        }
    }

    /// Run a batch of backtest tasks in parallel, respecting the concurrency limit.
    ///
    /// Returns results in the same order as the input tasks.
    pub async fn run_batch(
        &self,
        tasks: Vec<BacktestTask>,
    ) -> Vec<Result<BacktestResult, PoolError>> {
        let semaphore = Arc::new(Semaphore::new(self.concurrency));
        let mut handles = Vec::with_capacity(tasks.len());

        for task in tasks {
            let sem = semaphore.clone();
            let backtester = self.backtester.clone();
            let handle = tokio::spawn(async move {
                let _permit = sem.acquire().await.expect("semaphore not closed");
                backtester
                    .run(&task.strategy_artifact, &task.contract_id, task.timeframe)
                    .await
                    .map_err(|source| PoolError::TaskFailed {
                        task_id: task.id,
                        source,
                    })
            });
            handles.push(handle);
        }

        let mut results = Vec::with_capacity(handles.len());
        for handle in handles {
            let result = handle.await.unwrap_or_else(|e| {
                Err(PoolError::TaskFailed {
                    task_id: "unknown".to_string(),
                    source: BacktestError::ExecutionFailed {
                        message: format!("task panicked: {e}"),
                    },
                })
            });
            results.push(result);
        }
        results
    }

    /// Run a single backtest task.
    pub async fn run_single(
        &self,
        task: BacktestTask,
    ) -> Result<BacktestResult, PoolError> {
        self.backtester
            .run(&task.strategy_artifact, &task.contract_id, task.timeframe)
            .await
            .map_err(|source| PoolError::TaskFailed {
                task_id: task.id,
                source,
            })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};

    use async_trait::async_trait;
    use rust_decimal_macros::dec;

    use super::*;

    struct CountingBacktester {
        current: AtomicU32,
        max_concurrent: Arc<AtomicU32>,
    }

    impl CountingBacktester {
        fn new(max_concurrent: Arc<AtomicU32>) -> Self {
            Self {
                current: AtomicU32::new(0),
                max_concurrent,
            }
        }
    }

    #[async_trait]
    impl Backtester for CountingBacktester {
        async fn run(
            &self,
            _strategy_artifact: &[u8],
            _contract_id: &str,
            _timeframe: Timeframe,
        ) -> Result<BacktestResult, BacktestError> {
            let prev = self.current.fetch_add(1, Ordering::SeqCst);
            let running = prev + 1;
            // Update max if this is a new high-water mark
            self.max_concurrent
                .fetch_max(running, Ordering::SeqCst);

            tokio::time::sleep(std::time::Duration::from_millis(50)).await;

            self.current.fetch_sub(1, Ordering::SeqCst);

            Ok(BacktestResult::builder()
                .pnl(dec!(1000))
                .sharpe_ratio(1.5)
                .max_drawdown(dec!(0.1))
                .win_rate(0.6)
                .trade_count(100)
                .maybe_timeframe(None)
                .build())
        }
    }

    #[tokio::test]
    async fn batch_runs_in_parallel() {
        let max_concurrent = Arc::new(AtomicU32::new(0));
        let backtester = Arc::new(CountingBacktester::new(max_concurrent.clone()));
        let pool = BacktestPool::with_concurrency(backtester, 4);

        let tasks: Vec<BacktestTask> = (0..8)
            .map(|i| BacktestTask {
                id: format!("task-{i}"),
                strategy_artifact: Arc::new(vec![]),
                contract_id: "contract".to_string(),
                timeframe: Timeframe::Min1,
            })
            .collect();

        let results = pool.run_batch(tasks).await;

        assert_eq!(results.len(), 8);
        for r in &results {
            assert!(r.is_ok(), "expected Ok, got {r:?}");
        }
        assert!(
            max_concurrent.load(Ordering::SeqCst) >= 2,
            "expected at least 2 concurrent tasks, got {}",
            max_concurrent.load(Ordering::SeqCst)
        );
    }

    #[tokio::test]
    async fn single_run_works() {
        let max_concurrent = Arc::new(AtomicU32::new(0));
        let backtester = Arc::new(CountingBacktester::new(max_concurrent));
        let pool = BacktestPool::with_concurrency(backtester, 2);

        let task = BacktestTask {
            id: "single".to_string(),
            strategy_artifact: Arc::new(vec![]),
            contract_id: "contract".to_string(),
            timeframe: Timeframe::Min1,
        };

        let result = pool.run_single(task).await;
        assert!(result.is_ok());
    }
}
