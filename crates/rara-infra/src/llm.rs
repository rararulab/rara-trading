//! LLM client trait and error types.

use async_trait::async_trait;
use snafu::Snafu;

/// Errors from LLM client operations.
#[derive(Debug, Snafu)]
#[snafu(visibility(pub))]
pub enum LlmError {
    /// The LLM request failed.
    #[snafu(display("LLM request failed: {message}"))]
    RequestFailed {
        /// Description of the failure.
        message: String,
    },
}

/// Trait for interacting with a large language model.
#[async_trait]
pub trait LlmClient: Send + Sync {
    /// Send a prompt to the LLM and return the completion text.
    async fn complete(&self, prompt: &str) -> Result<String, LlmError>;
}
