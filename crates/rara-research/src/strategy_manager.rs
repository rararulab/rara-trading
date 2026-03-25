//! Strategy manager trait — owns the full strategy lifecycle.
//!
//! Abstracts code generation, compilation, error fixing, artifact loading,
//! and persistence so the research loop is runtime-agnostic.

use async_trait::async_trait;
use snafu::Snafu;
use uuid::Uuid;

use rara_domain::research::{Hypothesis, ResearchStrategy};

use crate::strategy_executor::StrategyHandle;
use crate::strategy_store::StrategyStore;

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
/// artifact loading, and provide access to the underlying store.
#[async_trait]
pub trait StrategyManager: Send + Sync {
    /// Generate strategy source code from a hypothesis.
    async fn generate_code(&self, hypothesis: &Hypothesis, context: &str) -> Result<String>;

    /// Compile source code into a binary artifact, returning the strategy record.
    ///
    /// Saves both the [`ResearchStrategy`] record and the compiled artifact.
    async fn compile(&self, hypothesis_id: Uuid, source_code: &str) -> Result<ResearchStrategy>;

    /// Fix compilation errors and return corrected source code.
    async fn fix_errors(
        &self,
        source_code: &str,
        errors: &[String],
        hypothesis: &Hypothesis,
    ) -> Result<String>;

    /// Load a strategy handle from a stored artifact by strategy ID.
    fn load_handle(&self, strategy_id: Uuid) -> Result<Box<dyn StrategyHandle>>;

    /// Access the underlying strategy store.
    fn store(&self) -> &StrategyStore;
}
