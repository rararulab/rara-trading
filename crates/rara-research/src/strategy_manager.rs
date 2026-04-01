//! Strategy manager trait — owns the full strategy lifecycle.
//!
//! Abstracts code generation, compilation, error fixing, artifact loading,
//! and persistence so the research loop is runtime-agnostic.

use async_trait::async_trait;
use rara_domain::research::{Hypothesis, ResearchStrategy, ResearchStrategyStatus};
use snafu::Snafu;
use uuid::Uuid;

use crate::strategy_executor::StrategyHandle;

/// Errors from strategy manager operations.
#[derive(Debug, Snafu)]
#[snafu(visibility(pub))]
pub enum StrategyManagerError {
    /// Code generation failed.
    #[snafu(display("code generation failed: {message}"))]
    CodeGen {
        /// Error details.
        message: String,
    },

    /// Compilation failed.
    #[snafu(display("compilation failed: {message}"))]
    Compile {
        /// Error details.
        message: String,
    },

    /// All compile retries exhausted.
    #[snafu(display("compilation failed after retries: {}", errors.join("; ")))]
    CompileFailed {
        /// The last set of compilation errors.
        errors: Vec<String>,
    },

    /// Artifact loading failed.
    #[snafu(display("load failed: {message}"))]
    Load {
        /// Error details.
        message: String,
    },

    /// Store operation failed.
    #[snafu(display("store error: {message}"))]
    Store {
        /// Error details.
        message: String,
    },
}

/// Alias for strategy manager results.
pub type Result<T> = std::result::Result<T, StrategyManagerError>;

/// Trait for managing the full strategy lifecycle.
///
/// Implementations own code generation, compilation, error fixing,
/// artifact loading, and strategy status management.
#[async_trait]
pub trait StrategyManager: Send + Sync {
    /// Generate strategy source code from a hypothesis.
    async fn generate_code(&self, hypothesis: &Hypothesis, context: &str) -> Result<String>;

    /// Try to compile source code, returning the artifact bytes on success
    /// or the list of errors on failure.
    ///
    /// Does NOT persist anything — use [`compile`](Self::compile) to persist
    /// the strategy record and artifact after a successful compilation.
    async fn try_compile(&self, source_code: &str) -> Result<Vec<u8>>;

    /// Persist a successfully compiled strategy and its artifact.
    ///
    /// Call this after [`try_compile`](Self::try_compile) succeeds.
    fn save_strategy(
        &self,
        hypothesis_id: Uuid,
        source_code: &str,
        artifact: &[u8],
    ) -> Result<ResearchStrategy>;

    /// Fix compilation errors and return corrected source code.
    async fn fix_errors(
        &self,
        source_code: &str,
        errors: &[String],
        hypothesis: &Hypothesis,
    ) -> Result<String>;

    /// Load a strategy handle from a stored artifact by strategy ID.
    fn load_handle(&self, strategy_id: Uuid) -> Result<Box<dyn StrategyHandle>>;

    /// Update the lifecycle status of a strategy.
    fn update_status(&self, strategy_id: Uuid, status: ResearchStrategyStatus) -> Result<()>;
}
