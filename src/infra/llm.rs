//! LLM client trait and mock implementation.

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

/// A mock LLM client that returns pre-configured responses for testing.
#[derive(Clone)]
pub struct MockLlmClient {
    responses: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
}

impl MockLlmClient {
    /// Create a new mock client with a queue of responses.
    pub fn new(responses: Vec<String>) -> Self {
        Self {
            responses: std::sync::Arc::new(std::sync::Mutex::new(responses)),
        }
    }
}

#[async_trait]
impl LlmClient for MockLlmClient {
    async fn complete(&self, _prompt: &str) -> Result<String, LlmError> {
        let mut queue = self.responses.lock().expect("mock lock poisoned");
        if queue.is_empty() {
            Ok("mock response".to_owned())
        } else {
            Ok(queue.remove(0))
        }
    }
}
