//! Research loop orchestration — the full propose -> code -> backtest -> evaluate cycle.

use std::sync::Arc;

use rust_decimal::Decimal;
use snafu::{ResultExt, Snafu};

use rara_domain::event::Event;
use rara_domain::research::{Experiment, Hypothesis, HypothesisFeedback};
use rara_event_bus::bus::EventBus;
use rara_infra::llm::LlmClient;

use crate::backtester::Backtester;
use crate::hypothesis_gen::HypothesisGenerator;
use crate::strategy_coder::StrategyCoder;
use crate::trace::Trace;

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
/// propose -> code -> backtest -> evaluate -> record.
pub struct ResearchLoop<L: LlmClient, B: Backtester> {
    hypothesis_gen: HypothesisGenerator<L>,
    strategy_coder: StrategyCoder<L>,
    backtester: B,
    trace: Trace,
    event_bus: Arc<EventBus>,
}

impl<L: LlmClient + Clone, B: Backtester> ResearchLoop<L, B> {
    /// Create a new research loop with all required components.
    pub fn new(
        llm: L,
        backtester: B,
        trace: Trace,
        event_bus: Arc<EventBus>,
    ) -> Self {
        Self {
            hypothesis_gen: HypothesisGenerator::new(llm.clone()),
            strategy_coder: StrategyCoder::new(llm),
            backtester,
            trace,
            event_bus,
        }
    }

    /// Run one full research iteration.
    ///
    /// Steps: generate hypothesis -> generate code -> backtest -> evaluate ->
    /// record feedback -> publish events.
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
        let code = self
            .strategy_coder
            .generate_code(&hypothesis, context)
            .await
            .context(StrategyCodingSnafu)?;

        // 5. Create and save experiment
        let experiment = Experiment::builder()
            .hypothesis_id(hypothesis.id())
            .strategy_code(&code)
            .build();

        self.trace
            .save_experiment(&experiment)
            .context(TraceSnafu)?;

        // 6. Run backtest
        let backtest_result = self
            .backtester
            .run(&code, "default")
            .await
            .context(BacktestSnafu)?;

        // 7. Evaluate: accept if sharpe > 1.0 and max_drawdown < 0.15
        let max_drawdown_threshold = Decimal::new(15, 2);
        let accepted = backtest_result.sharpe_ratio() > 1.0
            && backtest_result.max_drawdown() < max_drawdown_threshold;

        // 8. Create feedback
        let feedback = HypothesisFeedback::builder()
            .experiment_id(experiment.id())
            .decision(accepted)
            .reason(if accepted {
                format!(
                    "Accepted: sharpe={:.2}, max_drawdown={}",
                    backtest_result.sharpe_ratio(),
                    backtest_result.max_drawdown()
                )
            } else {
                format!(
                    "Rejected: sharpe={:.2}, max_drawdown={}",
                    backtest_result.sharpe_ratio(),
                    backtest_result.max_drawdown()
                )
            })
            .observations(format!(
                "pnl={}, win_rate={:.2}, trades={}",
                backtest_result.pnl(),
                backtest_result.win_rate(),
                backtest_result.trade_count()
            ))
            .build();

        // 9. Save feedback
        self.trace.save_feedback(&feedback).context(TraceSnafu)?;

        // 10. Publish experiment completed event
        self.publish_event(
            "research.experiment.completed",
            &serde_json::json!({
                "experiment_id": experiment.id().to_string(),
                "accepted": accepted,
            }),
        )?;

        // 11. If accepted, publish candidate event
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
