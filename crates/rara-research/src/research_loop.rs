//! Research loop orchestration — the full propose -> code -> compile -> backtest -> evaluate cycle.

use std::path::PathBuf;
use std::sync::Arc;

use bon::Builder;
use rust_decimal::Decimal;
use serde::Serialize;
use snafu::{ResultExt, Snafu};

use rara_domain::event::Event;
use rara_domain::timeframe::Timeframe;

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
    accepted: bool,
}

/// Event payload for a strategy candidate.
#[derive(Debug, Serialize)]
struct StrategyCandidatePayload {
    /// UUID of the accepted experiment.
    experiment_id: String,
    /// UUID of the originating hypothesis.
    hypothesis_id: String,
}
use rara_domain::research::{Experiment, Hypothesis, HypothesisFeedback};
use rara_event_bus::bus::EventBus;
use rara_infra::llm::LlmClient;

use crate::backtester::Backtester;
use crate::compiler::StrategyCompiler;
use crate::feedback_gen::FeedbackGenerator;
use crate::hypothesis_gen::HypothesisGenerator;
use crate::prompt_renderer::PromptRenderer;
use crate::strategy_executor::StrategyExecutor;
use crate::wasm_executor::WasmExecutor;
use crate::strategy_coder::StrategyCoder;
use crate::strategy_promoter::PromotedStrategy;
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
        source: crate::strategy_executor::ExecutorError,
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
    /// Strategy promotion failed.
    #[snafu(display("promotion failed: {source}"))]
    Promote {
        /// The underlying promoter error.
        source: crate::strategy_promoter::PromoterError,
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
    pub feedback: HypothesisFeedback,
    /// Whether the experiment was accepted.
    pub accepted: bool,
    /// Promoted strategy metadata, present when the experiment was accepted
    /// and auto-promotion is enabled.
    pub promoted: Option<PromotedStrategy>,
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
    runtime: WasmExecutor,
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
    /// Directory for promoted strategies. When set, accepted strategies are
    /// automatically saved here for paper trading pickup.
    promoted_dir: Option<PathBuf>,
    /// Directory for saving generated strategy source code each iteration.
    /// When set, each iteration's `.rs` source is persisted here for debugging
    /// and reproducibility.
    generated_dir: Option<PathBuf>,
    /// Optional market data cache for zero-copy backtest data loading.
    /// When present, backtest iterations use cached mmap'd data instead
    /// of loading from disk each time.
    data_cache: Option<Arc<rara_market_data::cache::DataCache>>,
    /// Timeframes to evaluate each hypothesis on.
    #[builder(default = vec![Timeframe::Hour1, Timeframe::Hour4, Timeframe::Day1])]
    timeframes: Vec<Timeframe>,
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
            &HypothesisCreatedPayload {
                hypothesis_id: hypothesis.id.to_string(),
            },
        )?;

        // 4. Generate strategy code
        let mut code = self
            .strategy_coder
            .generate_code(&hypothesis, context)
            .await
            .context(StrategyCodingSnafu)?;

        // 5. Save generated source to generated_dir for debugging/reproducibility
        if let Some(ref dir) = self.generated_dir {
            std::fs::create_dir_all(dir).context(IoSnafu)?;
            let path = dir.join(format!("{}.rs", hypothesis.id));
            std::fs::write(&path, &code).context(IoSnafu)?;
        }

        // 6. Compile to WASM with retries
        let wasm_bytes = self.compile_with_retries(&mut code, &hypothesis).await?;

        // 6. Load into executor to validate the module
        let _loaded = self.runtime.load(&wasm_bytes).context(RuntimeSnafu)?;

        // Keep wasm_bytes for potential promotion after acceptance
        let wasm_bytes_for_promotion = wasm_bytes;

        // 7. Create and save experiment
        let experiment = Experiment::builder()
            .hypothesis_id(hypothesis.id)
            .strategy_code(&code)
            .build();

        self.trace
            .save_experiment(&experiment)
            .context(TraceSnafu)?;

        // 8. Run backtests across all configured timeframes, pick best result
        let backtest_result = self
            .run_multi_timeframe_backtest(&wasm_bytes_for_promotion)
            .await?;

        // 9. Evaluate: accept if sharpe > 1.0 and max_drawdown < 0.15
        let max_drawdown_threshold = Decimal::new(15, 2);
        let accepted = backtest_result.sharpe_ratio > 1.0
            && backtest_result.max_drawdown < max_drawdown_threshold;

        // 10. Generate feedback via FeedbackGenerator
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

        // 11. Record in Trace DAG
        self.trace
            .record(&experiment, &feedback, &DagSelection::Latest)
            .context(TraceSnafu)?;

        // 12. Publish experiment completed event
        self.publish_event(
            "research.experiment.completed",
            &ExperimentCompletedPayload {
                experiment_id: experiment.id.to_string(),
                accepted,
            },
        )?;

        // 13. If accepted, publish candidate event and auto-promote
        let promoted = if accepted {
            self.publish_event(
                "research.strategy.candidate",
                &StrategyCandidatePayload {
                    experiment_id: experiment.id.to_string(),
                    hypothesis_id: hypothesis.id.to_string(),
                },
            )?;

            self.try_promote(
                experiment.id,
                hypothesis.id,
                &wasm_bytes_for_promotion,
                &code,
            )?
        } else {
            None
        };

        Ok(IterationResult {
            hypothesis,
            experiment,
            feedback,
            accepted,
            promoted,
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

    /// Run backtests across all configured timeframes, returning the best result.
    ///
    /// Each timeframe is tested sequentially. Failed individual timeframes are
    /// logged as warnings but do not abort the entire run. The result with the
    /// highest `sharpe_ratio` is returned. If all timeframes fail, the last
    /// error is propagated.
    async fn run_multi_timeframe_backtest(
        &self,
        wasm_bytes: &[u8],
    ) -> Result<rara_domain::research::BacktestResult> {
        let mut best: Option<rara_domain::research::BacktestResult> = None;
        let mut last_error: Option<ResearchLoopError> = None;

        for &timeframe in &self.timeframes {
            let result = self.run_backtest_single(wasm_bytes, timeframe).await;

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

    /// Run a single backtest for one timeframe, using the data cache when available.
    async fn run_backtest_single(
        &self,
        wasm_bytes: &[u8],
        timeframe: Timeframe,
    ) -> Result<rara_domain::research::BacktestResult> {
        if let Some(ref cache) = self.data_cache {
            let slices = cache
                .load_range(
                    "default",
                    rara_market_data::cache::DataType::Candle1m,
                    "2020-01-01", // TODO: make configurable
                    "2030-12-31",
                )
                .map_err(|e| ResearchLoopError::Backtest {
                    source: crate::backtester::BacktestError::ExecutionFailed {
                        message: format!("data cache error: {e}"),
                    },
                })?;
            self.backtester
                .run_with_data(wasm_bytes, "default", timeframe, &slices)
                .await
                .context(BacktestSnafu)
        } else {
            self.backtester
                .run(wasm_bytes, "default", timeframe)
                .await
                .context(BacktestSnafu)
        }
    }

    /// Auto-promote an accepted strategy if a promoted directory is configured.
    fn try_promote(
        &self,
        experiment_id: uuid::Uuid,
        hypothesis_id: uuid::Uuid,
        wasm_bytes: &[u8],
        source_code: &str,
    ) -> Result<Option<PromotedStrategy>> {
        let Some(ref promoted_dir) = self.promoted_dir else {
            return Ok(None);
        };

        let promoter = crate::strategy_promoter::StrategyPromoter::builder()
            .trace(
                crate::trace::Trace::open(
                    &promoted_dir
                        .parent()
                        .unwrap_or(promoted_dir)
                        .join("trace_promote"),
                )
                .context(TraceSnafu)?,
            )
            .runtime(WasmExecutor::builder().build())
            .compiler(
                StrategyCompiler::builder()
                    .template_dir(PathBuf::new())
                    .build(),
            )
            .promoted_dir(promoted_dir.clone())
            .build();

        let promoted_strategy = promoter
            .promote_from_wasm(experiment_id, hypothesis_id, wasm_bytes, Some(source_code))
            .context(PromoteSnafu)?;

        tracing::info!(
            %experiment_id,
            wasm_path = %promoted_strategy.wasm_path().display(),
            "strategy promoted for paper trading"
        );

        Ok(Some(promoted_strategy))
    }

    /// Helper to publish a domain event with a typed payload.
    fn publish_event(
        &self,
        event_type: &str,
        payload: &impl Serialize,
    ) -> Result<()> {
        let event = Event::builder()
            .event_type(event_type)
            .source("research_loop")
            .correlation_id(uuid::Uuid::new_v4().to_string())
            .payload(serde_json::to_value(payload).expect("event payload must serialize"))
            .build();
        self.event_bus.publish(&event).context(EventBusSnafu)?;
        Ok(())
    }
}
