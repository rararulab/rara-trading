//! Strategy code generation from hypotheses using an LLM.

use snafu::{ResultExt, Snafu};

use crate::domain::research::Hypothesis;
use crate::infra::llm::LlmClient;

/// Errors from strategy code generation.
#[derive(Debug, Snafu)]
#[snafu(visibility(pub))]
pub enum StrategyCoderError {
    /// LLM call failed.
    #[snafu(display("LLM error: {source}"))]
    Llm {
        /// The underlying LLM error.
        source: crate::infra::llm::LlmError,
    },
}

/// Alias for strategy coder results.
pub type Result<T> = std::result::Result<T, StrategyCoderError>;

/// Generates strategy source code from a hypothesis using an LLM.
pub struct StrategyCoder<L: LlmClient> {
    llm: L,
}

impl<L: LlmClient> StrategyCoder<L> {
    /// Create a new strategy coder backed by the given LLM client.
    pub const fn new(llm: L) -> Self {
        Self { llm }
    }

    /// Generate strategy code based on a hypothesis and additional context.
    pub async fn generate_code(
        &self,
        hypothesis: &Hypothesis,
        context: &str,
    ) -> Result<String> {
        let prompt = format!(
            "Generate trading strategy code for this hypothesis:\n\
             Hypothesis: {}\n\
             Reason: {}\n\
             Context: {context}\n\n\
             Return only the strategy code.",
            hypothesis.text(),
            hypothesis.reason()
        );

        self.llm.complete(&prompt).await.context(LlmSnafu)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infra::llm::MockLlmClient;

    #[tokio::test]
    async fn generate_code_returns_llm_response() {
        let mock = MockLlmClient::new(vec!["fn strategy() { buy() }".to_owned()]);
        let coder = StrategyCoder::new(mock);

        let h = Hypothesis::builder()
            .text("momentum works")
            .reason("historical evidence")
            .build();

        let code = coder.generate_code(&h, "BTC").await.unwrap();
        assert_eq!(code, "fn strategy() { buy() }");
    }
}
