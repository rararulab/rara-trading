//! LLM-driven feedback generator for evaluating experiment results.
//!
//! Renders the `feedback_gen` prompt template with experiment context,
//! calls the LLM, and parses the structured JSON response into
//! [`HypothesisFeedback`].

use std::collections::HashMap;

use snafu::{ResultExt, Snafu};
use uuid::Uuid;

use rara_domain::research::{BacktestResult, Hypothesis, HypothesisFeedback};
use rara_infra::llm::LlmClient;

use crate::prompt_renderer::{PromptError, PromptRenderer};

/// Errors from the feedback generator.
#[derive(Debug, Snafu)]
#[snafu(visibility(pub))]
pub enum FeedbackGenError {
    /// LLM call failed.
    #[snafu(display("LLM error: {source}"))]
    Llm {
        /// The underlying LLM error.
        source: rara_infra::llm::LlmError,
    },
    /// Prompt rendering failed.
    #[snafu(display("prompt error: {source}"))]
    Prompt {
        /// The underlying prompt error.
        source: PromptError,
    },
    /// LLM response could not be parsed as valid feedback JSON.
    #[snafu(display("failed to parse LLM response: {message}"))]
    Parse {
        /// Description of the parse failure.
        message: String,
    },
}

/// Alias for feedback generation results.
pub type Result<T> = std::result::Result<T, FeedbackGenError>;

/// Raw JSON structure matching the LLM response format.
#[derive(serde::Deserialize)]
struct RawFeedback {
    decision: bool,
    reason: String,
    observations: String,
    hypothesis_evaluation: String,
    new_hypothesis: Option<String>,
    code_change_summary: String,
}

/// Generates structured feedback for experiments by prompting an LLM.
pub struct FeedbackGenerator<L: LlmClient> {
    llm: L,
    prompt_renderer: PromptRenderer,
}

impl<L: LlmClient> FeedbackGenerator<L> {
    /// Create a new feedback generator with the given LLM client and prompt renderer.
    pub const fn new(llm: L, prompt_renderer: PromptRenderer) -> Self {
        Self {
            llm,
            prompt_renderer,
        }
    }

    /// Generate structured feedback for an experiment.
    ///
    /// Renders the `feedback_gen` prompt template with hypothesis text,
    /// backtest results, SOTA comparison, and strategy code, then calls
    /// the LLM and parses the JSON response into [`HypothesisFeedback`].
    pub async fn generate(
        &self,
        experiment_id: Uuid,
        hypothesis: &Hypothesis,
        backtest_result: &BacktestResult,
        strategy_code: &str,
        sota: Option<&BacktestResult>,
    ) -> Result<HypothesisFeedback> {
        let vars = Self::build_vars(hypothesis, backtest_result, strategy_code, sota);
        let prompt = self
            .prompt_renderer
            .render("feedback_gen", &vars)
            .context(PromptSnafu)?;
        let response = self.llm.complete(&prompt).await.context(LlmSnafu)?;
        let raw = Self::parse_response(&response)?;

        let feedback = HypothesisFeedback::builder()
            .experiment_id(experiment_id)
            .decision(raw.decision)
            .reason(raw.reason)
            .observations(raw.observations)
            .hypothesis_evaluation(raw.hypothesis_evaluation)
            .maybe_new_hypothesis(raw.new_hypothesis)
            .code_change_summary(raw.code_change_summary)
            .build();

        Ok(feedback)
    }

    /// Build template variables from experiment context.
    fn build_vars(
        hypothesis: &Hypothesis,
        backtest_result: &BacktestResult,
        strategy_code: &str,
        sota: Option<&BacktestResult>,
    ) -> HashMap<String, String> {
        let mut vars = HashMap::new();

        vars.insert("hypothesis".to_owned(), hypothesis.text.clone());
        vars.insert(
            "backtest_result".to_owned(),
            format_backtest_result(backtest_result),
        );
        vars.insert(
            "sota_result".to_owned(),
            sota.map_or_else(
                || "No previous SOTA available".to_owned(),
                format_backtest_result,
            ),
        );
        vars.insert("strategy_code".to_owned(), strategy_code.to_owned());

        vars
    }

    /// Parse the LLM response JSON, stripping any markdown code fences.
    fn parse_response(response: &str) -> Result<RawFeedback> {
        let json_str = extract_json(response);
        serde_json::from_str(json_str).map_err(|e| {
            ParseSnafu {
                message: format!("{e}"),
            }
            .build()
        })
    }
}

/// Format a backtest result as a human-readable summary string.
fn format_backtest_result(result: &BacktestResult) -> String {
    format!(
        "Sharpe: {:.2}, PnL: {}, Max Drawdown: {}, Win Rate: {:.1}%, Trades: {}",
        result.sharpe_ratio,
        result.pnl,
        result.max_drawdown,
        result.win_rate * 100.0,
        result.trade_count,
    )
}

/// Extract JSON from a response that may be wrapped in markdown code fences.
fn extract_json(response: &str) -> &str {
    let trimmed = response.trim();

    // Try to extract from ```json ... ``` fences
    if let Some(start) = trimmed.find("```json") {
        let after_fence = &trimmed[start + 7..];
        if let Some(end) = after_fence.find("```") {
            return after_fence[..end].trim();
        }
    }

    // Try to extract from ``` ... ``` fences
    if let Some(start) = trimmed.find("```") {
        let after_fence = &trimmed[start + 3..];
        if let Some(end) = after_fence.find("```") {
            return after_fence[..end].trim();
        }
    }

    trimmed
}

#[cfg(test)]
mod tests {
    use super::*;

    use async_trait::async_trait;
    use rara_infra::llm::LlmError;
    use rust_decimal_macros::dec;

    /// Mock LLM client that returns a fixed response.
    struct MockLlm {
        response: String,
    }

    #[async_trait]
    impl LlmClient for MockLlm {
        async fn complete(&self, _prompt: &str) -> std::result::Result<String, LlmError> {
            Ok(self.response.clone())
        }
    }

    /// Mock LLM client that always fails.
    struct FailingLlm;

    #[async_trait]
    impl LlmClient for FailingLlm {
        async fn complete(&self, _prompt: &str) -> std::result::Result<String, LlmError> {
            Err(rara_infra::llm::RequestFailedSnafu {
                message: "service unavailable",
            }
            .build())
        }
    }

    fn make_renderer() -> PromptRenderer {
        let prompt_dir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/prompts");
        PromptRenderer::load_from_dir(&prompt_dir).expect("prompts dir should exist")
    }

    fn sample_hypothesis() -> Hypothesis {
        Hypothesis::builder()
            .text("Mean reversion on BTC/USD 1h")
            .reason("High volatility suggests mean reversion opportunities")
            .build()
    }

    fn sample_backtest_result() -> BacktestResult {
        BacktestResult::builder()
            .pnl(dec!(500.00))
            .sharpe_ratio(2.1)
            .max_drawdown(dec!(50.00))
            .win_rate(0.65)
            .trade_count(42)
            .build()
    }

    fn sample_sota() -> BacktestResult {
        BacktestResult::builder()
            .pnl(dec!(300.00))
            .sharpe_ratio(1.5)
            .max_drawdown(dec!(80.00))
            .win_rate(0.55)
            .trade_count(38)
            .build()
    }

    const VALID_JSON: &str = r#"{
        "decision": true,
        "reason": "Improved Sharpe ratio from 1.5 to 2.1",
        "observations": "Strong win rate and lower drawdown",
        "hypothesis_evaluation": "Code properly tests mean reversion with Bollinger Bands",
        "new_hypothesis": "Try adding volume confirmation filter",
        "code_change_summary": "Switched from SMA to Bollinger Band mean reversion"
    }"#;

    #[tokio::test]
    async fn generate_parses_valid_json_response() {
        let llm = MockLlm {
            response: VALID_JSON.to_owned(),
        };
        let generator = FeedbackGenerator::new(llm, make_renderer());
        let experiment_id = Uuid::new_v4();

        let feedback = generator
            .generate(
                experiment_id,
                &sample_hypothesis(),
                &sample_backtest_result(),
                "fn strategy() {}",
                Some(&sample_sota()),
            )
            .await
            .expect("should parse valid JSON");

        assert_eq!(feedback.experiment_id, experiment_id);
        assert!(feedback.decision);
        assert_eq!(feedback.reason, "Improved Sharpe ratio from 1.5 to 2.1");
        assert_eq!(feedback.observations, "Strong win rate and lower drawdown");
        assert_eq!(
            feedback.new_hypothesis,
            Some("Try adding volume confirmation filter")
        );
    }

    #[tokio::test]
    async fn generate_parses_json_in_code_fences() {
        let fenced = format!("Here is my analysis:\n```json\n{VALID_JSON}\n```\n");
        let llm = MockLlm { response: fenced };
        let generator = FeedbackGenerator::new(llm, make_renderer());

        let feedback = generator
            .generate(
                Uuid::new_v4(),
                &sample_hypothesis(),
                &sample_backtest_result(),
                "fn strategy() {}",
                None,
            )
            .await
            .expect("should extract JSON from code fences");

        assert!(feedback.decision);
    }

    #[tokio::test]
    async fn generate_rejects_malformed_json() {
        let llm = MockLlm {
            response: "not valid json at all".to_owned(),
        };
        let generator = FeedbackGenerator::new(llm, make_renderer());

        let err = generator
            .generate(
                Uuid::new_v4(),
                &sample_hypothesis(),
                &sample_backtest_result(),
                "fn strategy() {}",
                None,
            )
            .await
            .unwrap_err();

        assert!(matches!(err, FeedbackGenError::Parse { .. }));
    }

    #[tokio::test]
    async fn generate_propagates_llm_error() {
        let generator = FeedbackGenerator::new(FailingLlm, make_renderer());

        let err = generator
            .generate(
                Uuid::new_v4(),
                &sample_hypothesis(),
                &sample_backtest_result(),
                "fn strategy() {}",
                None,
            )
            .await
            .unwrap_err();

        assert!(matches!(err, FeedbackGenError::Llm { .. }));
    }

    #[test]
    fn format_backtest_result_output() {
        let result = sample_backtest_result();
        let formatted = format_backtest_result(&result);
        assert_eq!(
            formatted,
            "Sharpe: 2.10, PnL: 500.00, Max Drawdown: 50.00, Win Rate: 65.0%, Trades: 42"
        );
    }

    #[test]
    fn extract_json_handles_plain_json() {
        let input = r#"{"decision": true}"#;
        assert_eq!(extract_json(input), input);
    }

    #[test]
    fn extract_json_handles_fenced_json() {
        let input = "```json\n{\"decision\": true}\n```";
        assert_eq!(extract_json(input), "{\"decision\": true}");
    }

    #[test]
    fn extract_json_handles_bare_fences() {
        let input = "```\n{\"decision\": true}\n```";
        assert_eq!(extract_json(input), "{\"decision\": true}");
    }

    #[test]
    fn generate_uses_no_sota_text_when_none() {
        let vars = FeedbackGenerator::<MockLlm>::build_vars(
            &sample_hypothesis(),
            &sample_backtest_result(),
            "fn strategy() {}",
            None,
        );
        assert_eq!(vars["sota_result"], "No previous SOTA available");
    }
}
