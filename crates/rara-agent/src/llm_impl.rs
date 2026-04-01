//! `LlmClient` implementation for `CliExecutor`.

use async_trait::async_trait;
use rara_infra::llm::{LlmClient, LlmError};

use crate::executor::CliExecutor;

#[async_trait]
impl LlmClient for CliExecutor {
    async fn complete(&self, prompt: &str) -> Result<String, LlmError> {
        let result = self
            .execute_capture(prompt)
            .await
            .map_err(|e| LlmError::RequestFailed {
                message: e.to_string(),
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
