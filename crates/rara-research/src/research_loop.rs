//! Research loop orchestration — the full propose -> code -> compile ->
//! backtest -> evaluate cycle.

use std::{path::PathBuf, sync::Arc};

use bon::Builder;
use rara_domain::{
    event::{Event, EventType},
    research::{
        Experiment, Hypothesis, HypothesisFeedback, ResearchStrategy, ResearchStrategyStatus,
    },
    timeframe::Timeframe,
};
use rara_event_bus::bus::EventBus;
use rust_decimal::Decimal;
use serde::Serialize;
use snafu::{ResultExt, Snafu};
use uuid::Uuid;

use crate::{
    backtester::Backtester,
    feedback_gen::FeedbackGenerator,
    hypothesis_gen::HypothesisGenerator,
    prompt_renderer::PromptRenderer,
    strategy_manager::{StrategyManager, StrategyManagerError},
    trace::{DagSelection, Trace},
};

/// Event payload for hypothesis creation.
#[derive(Debug, Serialize)]
struct HypothesisCreatedPayload {
    /// UUID of the created hypothesis.
    hypothesis_id: String,
}

/// Event payload for experiment completion.
#[derive(Debug, Serialize)]
struct ExperimentCompletedPayload {
    /// UUID of the completed experiment.
    experiment_id: String,
    /// Whether the experiment met acceptance criteria.
    accepted:      bool,
}

/// Event payload for a strategy candidate.
#[derive(Debug, Serialize)]
struct StrategyCandidatePayload {
    /// UUID of the accepted experiment.
    experiment_id: String,
    /// UUID of the originating hypothesis.
    hypothesis_id: String,
}

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
    /// Strategy manager operation failed.
    #[snafu(display("strategy manager error: {source}"))]
    StrategyManager {
        /// The underlying error.
        source: StrategyManagerError,
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
    /// Filesystem I/O failed.
    #[snafu(display("I/O error: {source}"))]
    Io {
        /// The underlying I/O error.
        source: std::io::Error,
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
    pub feedback:   HypothesisFeedback,
    /// Whether the experiment was accepted.
    pub accepted:   bool,
    /// The compiled research strategy.
    pub strategy:   ResearchStrategy,
}

/// Orchestrates the full RD-Agent style research loop:
/// propose -> code -> compile -> backtest -> evaluate -> record.
#[derive(Builder)]
pub struct ResearchLoop {
    /// Generates new hypotheses from trace history.
    hypothesis_gen:      HypothesisGenerator,
    /// Manages the full strategy lifecycle (code gen, compile, load).
    strategy_manager:    Arc<dyn StrategyManager>,
    /// Runs backtests against loaded strategy handles.
    backtester:          Arc<dyn Backtester>,
    /// LLM-driven feedback evaluator.
    feedback_gen:        FeedbackGenerator,
    /// Prompt template renderer.
    #[allow(dead_code)]
    prompt_renderer:     PromptRenderer,
    /// DAG trace storage.
    trace:               Trace,
    /// Domain event bus.
    event_bus:           Arc<EventBus>,
    /// Maximum attempts to fix compile errors before giving up.
    #[builder(default = 3)]
    max_compile_retries: u32,
    /// Directory for saving generated strategy source code each iteration.
    /// When set, each iteration's `.rs` source is persisted here for debugging
    /// and reproducibility.
    generated_dir:       Option<PathBuf>,
    /// Timeframes to evaluate each hypothesis on.
    #[builder(default = vec![Timeframe::Hour1, Timeframe::Hour4, Timeframe::Day1])]
    timeframes:          Vec<Timeframe>,
}

impl ResearchLoop {
    /// Run one full research iteration.
    ///
    /// Steps: generate hypothesis -> generate code -> compile ->
    /// load into runtime -> backtest -> generate feedback -> record in DAG ->
    /// publish events.
    #[tracing::instrument(skip(self, context), fields(context_len = context.len()))]
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

        // 3. Publish hypothesis created event — establishes the correlation_id for this
        //    run
        let correlation_id = hypothesis.correlation_id.clone();
        self.publish_event(
            EventType::ResearchHypothesisCreated,
            &HypothesisCreatedPayload {
                hypothesis_id: hypothesis.id.to_string(),
            },
            &correlation_id,
        )?;

        // 4. Generate strategy code
        let mut code = self
            .strategy_manager
            .generate_code(&hypothesis, context)
            .await
            .context(StrategyManagerSnafu)?;

        // 5. Save generated source to generated_dir for debugging/reproducibility
        if let Some(ref dir) = self.generated_dir {
            std::fs::create_dir_all(dir).context(IoSnafu)?;
            let path = dir.join(format!("{}.rs", hypothesis.id));
            std::fs::write(&path, &code).context(IoSnafu)?;
        }

        // 6. Compile with retries, producing a persisted ResearchStrategy
        let strategy = self.compile_with_retries(&mut code, &hypothesis).await?;

        // 7. Validate the module loads successfully
        let _loaded = self
            .strategy_manager
            .load_handle(strategy.id)
            .context(StrategyManagerSnafu)?;

        // 8. Create experiment and run backtests
        let mut experiment = Experiment::builder()
            .hypothesis_id(hypothesis.id)
            .strategy_code(&code)
            .build();

        // 9. Run backtests across all configured timeframes, pick best result
        let backtest_result = self.run_multi_timeframe_backtest(strategy.id).await?;

        // 10. Persist experiment with backtest result attached
        experiment.backtest_result = Some(backtest_result.clone());
        self.trace
            .save_experiment(&experiment)
            .context(TraceSnafu)?;

        // 11. Evaluate: accept if sharpe > 1.0 and max_drawdown < 0.15
        let max_drawdown_threshold = Decimal::new(15, 2);
        let accepted = backtest_result.sharpe_ratio > 1.0
            && backtest_result.max_drawdown < max_drawdown_threshold;

        // 12. Generate feedback via FeedbackGenerator
        let sota_result = self
            .trace
            .get_sota()
            .context(TraceSnafu)?
            .and_then(|(exp, _)| exp.backtest_result);

        let feedback = self
            .feedback_gen
            .generate(
                experiment.id,
                &hypothesis,
                &backtest_result,
                &code,
                sota_result.as_ref(),
            )
            .await
            .context(FeedbackGenSnafu)?;

        // 13. Record in Trace DAG
        self.trace
            .record(&experiment, &feedback, &DagSelection::Latest)
            .context(TraceSnafu)?;

        // 14. Publish experiment completed event
        self.publish_event(
            EventType::ResearchExperimentCompleted,
            &ExperimentCompletedPayload {
                experiment_id: experiment.id.to_string(),
                accepted,
            },
            &correlation_id,
        )?;

        // 15. If accepted, update status and publish candidate event
        if accepted {
            self.strategy_manager
                .update_status(strategy.id, ResearchStrategyStatus::Accepted)
                .context(StrategyManagerSnafu)?;

            self.publish_event(
                EventType::ResearchStrategyCandidate,
                &StrategyCandidatePayload {
                    experiment_id: experiment.id.to_string(),
                    hypothesis_id: hypothesis.id.to_string(),
                },
                &correlation_id,
            )?;
        }

        Ok(IterationResult {
            hypothesis,
            experiment,
            feedback,
            accepted,
            strategy,
        })
    }

    /// Attempt to compile strategy code, retrying with LLM-driven fixes on
    /// failure.
    ///
    /// Only persists the strategy record and artifact after a successful
    /// compilation, avoiding orphaned records from failed attempts.
    #[tracing::instrument(skip(self, code, hypothesis), fields(hypothesis_id = %hypothesis.id))]
    async fn compile_with_retries(
        &self,
        code: &mut String,
        hypothesis: &Hypothesis,
    ) -> Result<ResearchStrategy> {
        let mut last_errors = vec![];

        for attempt in 0..=self.max_compile_retries {
            match self.strategy_manager.try_compile(code).await {
                Ok(artifact) => {
                    // Compilation succeeded — persist strategy + artifact
                    let strategy = self
                        .strategy_manager
                        .save_strategy(hypothesis.id, code, &artifact)
                        .context(StrategyManagerSnafu)?;
                    return Ok(strategy);
                }
                Err(StrategyManagerError::CompileFailed { errors }) => {
                    last_errors = errors;
                }
                Err(e) => return Err(ResearchLoopError::StrategyManager { source: e }),
            }

            // If we have retries left, ask the LLM to fix the errors
            if attempt < self.max_compile_retries {
                *code = self
                    .strategy_manager
                    .fix_errors(code, &last_errors, hypothesis)
                    .await
                    .context(StrategyManagerSnafu)?;
            }
        }

        Err(ResearchLoopError::StrategyManager {
            source: StrategyManagerError::CompileFailed {
                errors: last_errors,
            },
        })
    }

    /// Run backtests across all configured timeframes, returning the best
    /// result.
    ///
    /// Each timeframe is tested sequentially. Failed individual timeframes are
    /// logged as warnings but do not abort the entire run. The result with the
    /// highest `sharpe_ratio` is returned. If all timeframes fail, the last
    /// error is propagated.
    async fn run_multi_timeframe_backtest(
        &self,
        strategy_id: Uuid,
    ) -> Result<rara_domain::research::BacktestResult> {
        let mut best: Option<rara_domain::research::BacktestResult> = None;
        let mut last_error: Option<ResearchLoopError> = None;

        for &timeframe in &self.timeframes {
            let result = self.run_backtest_single(strategy_id, timeframe).await;

            match result {
                Ok(bt) => {
                    tracing::info!(%timeframe, sharpe = bt.sharpe_ratio, "backtest completed");
                    let is_better = best
                        .as_ref()
                        .is_none_or(|prev| bt.sharpe_ratio > prev.sharpe_ratio);
                    if is_better {
                        best = Some(bt);
                    }
                }
                Err(e) => {
                    tracing::warn!(%timeframe, error = %e, "backtest failed for timeframe");
                    last_error = Some(e);
                }
            }
        }

        best.ok_or_else(|| {
            last_error.unwrap_or_else(|| ResearchLoopError::Backtest {
                source: crate::backtester::BacktestError::ExecutionFailed {
                    message: "no timeframes configured".to_string(),
                },
            })
        })
    }

    /// Run a single backtest for one timeframe.
    async fn run_backtest_single(
        &self,
        strategy_id: Uuid,
        timeframe: Timeframe,
    ) -> Result<rara_domain::research::BacktestResult> {
        let handle = self
            .strategy_manager
            .load_handle(strategy_id)
            .context(StrategyManagerSnafu)?;
        self.backtester
            .run(handle, "default", timeframe)
            .await
            .context(BacktestSnafu)
    }

    /// Helper to publish a domain event with a typed payload.
    ///
    /// All events within a single pipeline run share the same `correlation_id`,
    /// which is generated once when the hypothesis is created and passed here.
    fn publish_event(
        &self,
        event_type: EventType,
        payload: &impl Serialize,
        correlation_id: &str,
    ) -> Result<()> {
        let event = Event::builder()
            .event_type(event_type)
            .source("research_loop")
            .correlation_id(correlation_id)
            .payload(serde_json::to_value(payload).expect("event payload must serialize"))
            .build();
        self.event_bus.publish(&event).context(EventBusSnafu)?;
        Ok(())
    }
}
