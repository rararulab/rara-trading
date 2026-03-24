//! LLM client trait and CLI-based implementation.

use async_trait::async_trait;
use snafu::Snafu;

use crate::agent::executor::CliExecutor;

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

#[async_trait]
impl LlmClient for CliExecutor {
    async fn complete(&self, prompt: &str) -> Result<String, LlmError> {
        let result = self.execute_capture(prompt).await.map_err(|e| {
            LlmError::RequestFailed {
                message: e.to_string(),
            }
        })?;
        if result.success {
            Ok(result.output.trim().to_owned())
        } else {
            Err(LlmError::RequestFailed {
                message: format!(
                    "CLI exited with code {:?}: {}",
                    result.exit_code,
                    result.stderr.trim()
                ),
            })
        }
    }
}
