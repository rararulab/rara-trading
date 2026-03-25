//! Abstract strategy execution interface, decoupled from any specific runtime (WASM, Python, etc.).

use rara_strategy_api::{Candle, RiskLevels, Side, Signal, StrategyMeta};
use snafu::Snafu;

/// Errors from strategy execution.
#[derive(Debug, Snafu)]
#[snafu(visibility(pub))]
pub enum ExecutorError {
    /// Failed to load strategy artifact.
    #[snafu(display("failed to load strategy: {message}"))]
    Load {
        /// Description of the load failure.
        message: String,
    },
    /// Strategy execution failed.
    #[snafu(display("strategy execution error: {message}"))]
    Execution {
        /// Description of the execution failure.
        message: String,
    },
}

/// Result alias for executor operations.
pub type Result<T> = std::result::Result<T, ExecutorError>;

/// Factory for loading compiled strategy artifacts into executable handles.
///
/// Implementations wrap a specific runtime (WASM, Python, etc.) and produce
/// [`StrategyHandle`] instances from compiled bytes.
pub trait StrategyExecutor: Send + Sync {
    /// Load a compiled strategy artifact, returning an executable handle.
    fn load(&self, artifact: &[u8]) -> Result<Box<dyn StrategyHandle>>;
}

/// Executable strategy handle — runtime-agnostic interface for calling strategy functions.
pub trait StrategyHandle: Send {
    /// Return strategy metadata.
    fn meta(&mut self) -> Result<StrategyMeta>;
    /// Process candle history and return a trading signal.
    fn on_candles(&mut self, candles: &[Candle]) -> Result<Signal>;
    /// Compute risk levels for a position entry.
    fn risk_levels(&mut self, entry_price: f64, side: Side) -> Result<RiskLevels>;
}
