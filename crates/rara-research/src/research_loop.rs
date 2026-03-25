//! Research loop orchestration — the full propose -> code -> compile -> backtest -> evaluate cycle.

use std::sync::Arc;

use bon::Builder;
use rust_decimal::Decimal;
use snafu::{ResultExt, Snafu};

use rara_domain::event::Event;
use rara_domain::research::{Experiment, Hypothesis, HypothesisFeedback};
use rara_event_bus::bus::EventBus;
use rara_infra::llm::LlmClient;

use crate::backtester::Backtester;
use crate::compiler::StrategyCompiler;
use crate::feedback_gen::FeedbackGenerator;
use crate::hypothesis_gen::HypothesisGenerator;
use crate::prompt_renderer::PromptRenderer;
use crate::runtime::StrategyRuntime;
use crate::strategy_coder::StrategyCoder;
use crate::trace::{DagSelection, Trace};

/// Errors from research loop execution.
#[derive(Debug, Snafu)]
#[snafu(visibility(pub))]
pub enum ResearchLoopError {
    /// Hypothesis generation failed.
    #[snafu(display("hypothesis generation failed: {source}"))]
    HypothesisGen {
        /// The underlying error.
        source: crate::hypothesis_gen::HypothesisGenError,
    },
    /// Strategy code generation failed.
    #[snafu(display("strategy coding failed: {source}"))]
    StrategyCoding {
        /// The underlying error.
        source: crate::strategy_coder::StrategyCoderError,
    },
    /// Strategy compilation failed.
    #[snafu(display("compilation failed: {source}"))]
    Compile {
        /// The underlying compiler error.
        source: crate::compiler::CompilerError,
    },
    /// All compile retries exhausted.
    #[snafu(display("compilation failed after retries: {}", errors.join("; ")))]
    CompileFailed {
        /// The last set of compilation errors.
        errors: Vec<String>,
    },
    /// WASM runtime error.
    #[snafu(display("runtime error: {source}"))]
    Runtime {
        /// The underlying runtime error.
        source: crate::runtime::RuntimeError,
    },
    /// Feedback generation failed.
    #[snafu(display("feedback generation failed: {source}"))]
    FeedbackGen {
        /// The underlying feedback generator error.
        source: crate::feedback_gen::FeedbackGenError,
    },
    /// Backtesting failed.
    #[snafu(display("backtesting failed: {source}"))]
    Backtest {
        /// The underlying error.
        source: crate::backtester::BacktestError,
    },
    /// Trace storage failed.
    #[snafu(display("trace error: {source}"))]
    Trace {
        /// The underlying trace error.
        source: crate::trace::TraceError,
    },
    /// Event publishing failed.
    #[snafu(display("event bus error: {source}"))]
    EventBus {
        /// The underlying store error.
        source: rara_event_bus::store::StoreError,
    },
}

/// Alias for research loop results.
pub type Result<T> = std::result::Result<T, ResearchLoopError>;

/// The outcome of a single research iteration.
pub struct IterationResult {
    /// The hypothesis that was tested.
    pub hypothesis: Hypothesis,
    /// The experiment that was run.
    pub experiment: Experiment,
    /// The feedback on the experiment.
    pub feedback: HypothesisFeedback,
    /// Whether the experiment was accepted.
    pub accepted: bool,
}

/// Orchestrates the full RD-Agent style research loop:
/// propose -> code -> compile -> backtest -> evaluate -> record.
#[derive(Builder)]
pub struct ResearchLoop<L: LlmClient, B: Backtester> {
    /// Generates new hypotheses from trace history.
    hypothesis_gen: HypothesisGenerator<L>,
    /// Generates and fixes strategy source code.
    strategy_coder: StrategyCoder<L>,
    /// Compiles strategy code to WASM.
    compiler: StrategyCompiler,
    /// Loads and validates compiled WASM modules.
    runtime: StrategyRuntime,
    /// Runs backtests against strategy code.
    backtester: B,
    /// LLM-driven feedback evaluator.
    feedback_gen: FeedbackGenerator<L>,
    /// Prompt template renderer (shared with `FeedbackGenerator` for other uses).
    #[allow(dead_code)]
    prompt_renderer: PromptRenderer,
    /// DAG trace storage.
    trace: Trace,
    /// Domain event bus.
    event_bus: Arc<EventBus>,
    /// Maximum attempts to fix compile errors before giving up.
    #[builder(default = 3)]
    max_compile_retries: u32,
}

impl<L: LlmClient + Clone, B: Backtester> ResearchLoop<L, B> {
    /// Run one full research iteration.
    ///
    /// Steps: generate hypothesis -> generate code -> compile to WASM ->
    /// load into runtime -> backtest -> generate feedback -> record in DAG ->
    /// publish events.
    pub async fn run_iteration(&self, context: &str) -> Result<IterationResult> {
        // 1. Generate hypothesis
        let hypothesis = self
            .hypothesis_gen
            .generate(&self.trace, context)
            .await
            .context(HypothesisGenSnafu)?;

        // 2. Save hypothesis
        self.trace
            .save_hypothesis(&hypothesis)
            .context(TraceSnafu)?;

        // 3. Publish hypothesis created event
        self.publish_event(
            "research.hypothesis.created",
            &serde_json::json!({ "hypothesis_id": hypothesis.id().to_string() }),
        )?;

        // 4. Generate strategy code
        let mut code = self
            .strategy_coder
            .generate_code(&hypothesis, context)
            .await
            .context(StrategyCodingSnafu)?;

        // 5. Compile to WASM with retries
        let wasm_bytes = self.compile_with_retries(&mut code, &hypothesis).await?;

        // 6. Load into StrategyRuntime to validate the module
        let _loaded = self.runtime.load(&wasm_bytes).context(RuntimeSnafu)?;

        // 7. Create and save experiment
        let experiment = Experiment::builder()
            .hypothesis_id(hypothesis.id())
            .strategy_code(&code)
            .build();

        self.trace
            .save_experiment(&experiment)
            .context(TraceSnafu)?;

        // 8. Run backtest (still using code string; WASM backtesting is Phase 4)
        let backtest_result = self
            .backtester
            .run(&code, "default")
            .await
            .context(BacktestSnafu)?;

        // 9. Evaluate: accept if sharpe > 1.0 and max_drawdown < 0.15
        let max_drawdown_threshold = Decimal::new(15, 2);
        let accepted = backtest_result.sharpe_ratio() > 1.0
            && backtest_result.max_drawdown() < max_drawdown_threshold;

        // 10. Generate feedback via FeedbackGenerator
        let sota_result = self
            .trace
            .get_sota()
            .context(TraceSnafu)?
            .and_then(|(exp, _)| exp.backtest_result().cloned());

        let feedback = self
            .feedback_gen
            .generate(
                experiment.id(),
                &hypothesis,
                &backtest_result,
                &code,
                sota_result.as_ref(),
            )
            .await
            .context(FeedbackGenSnafu)?;

        // 11. Record in Trace DAG
        self.trace
            .record(&experiment, &feedback, &DagSelection::Latest)
            .context(TraceSnafu)?;

        // 12. Publish experiment completed event
        self.publish_event(
            "research.experiment.completed",
            &serde_json::json!({
                "experiment_id": experiment.id().to_string(),
                "accepted": accepted,
            }),
        )?;

        // 13. If accepted, publish candidate event
        if accepted {
            self.publish_event(
                "research.strategy.candidate",
                &serde_json::json!({
                    "experiment_id": experiment.id().to_string(),
                    "hypothesis_id": hypothesis.id().to_string(),
                }),
            )?;
        }

        Ok(IterationResult {
            hypothesis,
            experiment,
            feedback,
            accepted,
        })
    }

    /// Attempt to compile strategy code, retrying with LLM-driven fixes on failure.
    async fn compile_with_retries(
        &self,
        code: &mut String,
        hypothesis: &Hypothesis,
    ) -> Result<Vec<u8>> {
        let mut last_errors = vec![];

        for attempt in 0..=self.max_compile_retries {
            let result = self.compiler.compile(code).await.context(CompileSnafu)?;

            if result.success {
                return Ok(result.wasm_bytes.expect("success implies wasm_bytes"));
            }

            last_errors = result.errors;

            // If we have retries left, ask the LLM to fix the errors
            if attempt < self.max_compile_retries {
                *code = self
                    .strategy_coder
                    .fix_errors(code, &last_errors, hypothesis)
                    .await
                    .context(StrategyCodingSnafu)?;
            }
        }

        Err(ResearchLoopError::CompileFailed {
            errors: last_errors,
        })
    }

    /// Helper to publish a domain event.
    fn publish_event(
        &self,
        event_type: &str,
        payload: &serde_json::Value,
    ) -> Result<()> {
        let event = Event::builder()
            .event_type(event_type)
            .source("research_loop")
            .correlation_id(uuid::Uuid::new_v4().to_string())
            .payload(payload.clone())
            .build();
        self.event_bus.publish(&event).context(EventBusSnafu)?;
        Ok(())
    }
}
