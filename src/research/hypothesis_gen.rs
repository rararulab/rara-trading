//! Hypothesis generation using LLM prompts informed by trace history.

use snafu::{ResultExt, Snafu};

use crate::domain::research::Hypothesis;
use crate::infra::llm::LlmClient;

use super::trace::Trace;

/// Errors from hypothesis generation.
#[derive(Debug, Snafu)]
#[snafu(visibility(pub))]
pub enum HypothesisGenError {
    /// LLM call failed.
    #[snafu(display("LLM error: {source}"))]
    Llm {
        /// The underlying LLM error.
        source: crate::infra::llm::LlmError,
    },
    /// Trace lookup failed.
    #[snafu(display("trace error: {source}"))]
    Trace {
        /// The underlying trace error.
        source: super::trace::TraceError,
    },
    /// LLM response could not be parsed into a hypothesis.
    #[snafu(display("failed to parse LLM response: {message}"))]
    Parse {
        /// Description of the parse failure.
        message: String,
    },
}

/// Alias for hypothesis generation results.
pub type Result<T> = std::result::Result<T, HypothesisGenError>;

/// Generates new hypotheses by prompting an LLM with trace context.
pub struct HypothesisGenerator<L: LlmClient> {
    llm: L,
}

impl<L: LlmClient> HypothesisGenerator<L> {
    /// Create a new generator backed by the given LLM client.
    pub const fn new(llm: L) -> Self {
        Self { llm }
    }

    /// Generate a new hypothesis informed by the trace history and context.
    ///
    /// The LLM response is parsed as: first line = hypothesis text,
    /// second line = reasoning.
    pub async fn generate(&self, trace: &Trace, context: &str) -> Result<Hypothesis> {
        let prompt = Self::build_prompt(trace, context)?;
        let response = self.llm.complete(&prompt).await.context(LlmSnafu)?;
        Self::parse_response(&response, trace)
    }

    /// Build a prompt incorporating trace history for the LLM.
    fn build_prompt(trace: &Trace, context: &str) -> Result<String> {
        use std::fmt::Write;

        let mut prompt = String::from("Generate a trading hypothesis.\n\n");

        if let Some((exp, fb)) = trace.get_best_experiment().context(TraceSnafu)? {
            let _ = write!(
                prompt,
                "Best experiment so far:\n- Code: {}\n- Feedback: {}\n\n",
                exp.strategy_code(),
                fb.reason()
            );
        }

        let _ = write!(prompt, "Context: {context}\n\n");
        prompt.push_str(
            "Respond with exactly two lines:\nLine 1: hypothesis text\nLine 2: reasoning",
        );

        Ok(prompt)
    }

    /// Parse the LLM response into a Hypothesis, linking to the best
    /// experiment's hypothesis as parent if one exists.
    fn parse_response(
        response: &str,
        trace: &Trace,
    ) -> Result<Hypothesis> {
        let lines: Vec<&str> = response.lines().collect();

        let text = lines
            .first()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| ParseSnafu { message: "empty response".to_owned() }.build())?;

        let reason = lines.get(1).unwrap_or(&"no reason provided");

        // Link to the best experiment's hypothesis as parent
        let parent_id = trace
            .get_best_experiment()
            .context(TraceSnafu)?
            .map(|(exp, _)| exp.hypothesis_id());

        let hypothesis = Hypothesis::builder()
            .text(*text)
            .reason(*reason)
            .maybe_parent(parent_id)
            .build();

        Ok(hypothesis)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::research::Experiment;
    use crate::infra::llm::MockLlmClient;

    #[tokio::test]
    async fn generate_links_parent_to_best_experiment() {
        let dir = tempfile::tempdir().unwrap();
        let trace = Trace::open(dir.path()).unwrap();

        // Set up a prior accepted experiment
        let parent_hyp = Hypothesis::builder()
            .text("parent hypothesis")
            .reason("parent reason")
            .build();
        trace.save_hypothesis(&parent_hyp).unwrap();

        let exp = Experiment::builder()
            .hypothesis_id(parent_hyp.id())
            .strategy_code("fn run() {}")
            .build();
        trace.save_experiment(&exp).unwrap();

        let fb = crate::domain::research::HypothesisFeedback::builder()
            .experiment_id(exp.id())
            .decision(true)
            .reason("good")
            .observations("ok")
            .build();
        trace.save_feedback(&fb).unwrap();

        let mock = MockLlmClient::new(vec!["momentum crossover\nSMA cross signals trend reversal".to_owned()]);
        let generator = HypothesisGenerator::new(mock);

        let h = generator.generate(&trace, "BTC market").await.unwrap();
        assert_eq!(h.text(), "momentum crossover");
        assert_eq!(h.reason(), "SMA cross signals trend reversal");
        assert_eq!(h.parent(), Some(parent_hyp.id()));
    }
}
