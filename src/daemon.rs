//! Unified daemon orchestrator — runs research, paper trading, feedback, and
//! gRPC server as concurrent tokio tasks in a single process.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::NaiveDate;
use rust_decimal_macros::dec;
use snafu::ResultExt;
use tokio::task::JoinSet;
use tracing::{error, info, warn};

use crate::accounts_config;
use crate::agent::{CliBackend, CliExecutor};
use crate::app_config;
use crate::error::{self, AgentBackendSnafu, EventBusSnafu, MarketStoreSnafu, PromptRendererSnafu, TraceSnafu};
use crate::event_bus::bus::EventBus;
use crate::paths;
use crate::research::barter_backtester::BarterBacktester;
use crate::research::compiler::StrategyCompiler;
use crate::research::feedback_gen::FeedbackGenerator;
use crate::research::hypothesis_gen::HypothesisGenerator;
use crate::research::prompt_renderer::PromptRenderer;
use crate::research::research_loop::ResearchLoop;
use crate::research::strategy_coder::StrategyCoder;
use crate::research::strategy_store::StrategyStore;
use crate::research::trace::Trace;
use crate::research::wasm_executor::WasmExecutor;
use crate::research::wasm_strategy_manager::WasmStrategyManager;

use rara_domain::event::EventType;

/// Run the unified daemon: spawn all trading-loop components as concurrent
/// tokio tasks and wait for shutdown (Ctrl+C) or a fatal task error.
///
/// Accounts and contracts are loaded from `accounts.toml` rather than CLI flags.
pub async fn run(iterations: u32, grpc_addr: String) -> error::Result<()> {
    // Load accounts from config; collect contracts from all enabled accounts
    let accounts_cfg = accounts_config::load_accounts();
    let _account_manager = rara_trading_engine::account_manager::AccountManager::from_config(
        &accounts_cfg.accounts,
    )
    .expect("failed to initialize accounts from config");

    // Persistent event bus shared across all components
    let trace_path = paths::data_dir().join("trace");
    let event_bus = Arc::new(EventBus::open(&trace_path.join("events")).context(EventBusSnafu)?);

    let contract_list: Vec<String> = accounts_cfg
        .accounts
        .iter()
        .filter(|a| a.enabled)
        .flat_map(|a| a.contracts.clone())
        .collect();

    info!(
        contracts = ?contract_list,
        iterations,
        grpc_addr = %grpc_addr,
        "daemon starting"
    );

    let mut tasks = JoinSet::new();

    // --- gRPC server task ---
    // TODO: spawn rara-server gRPC service once the crate is available on main.
    // Expected usage:
    //   let addr = grpc_addr.parse().expect("valid socket addr");
    //   let svc = rara_server::build_service(Arc::clone(&event_bus));
    //   tasks.spawn(async move { tonic::transport::Server::builder()
    //       .add_service(svc).serve(addr).await });
    let grpc_addr_clone = grpc_addr.clone();
    tasks.spawn(async move {
        info!(addr = %grpc_addr_clone, "gRPC server placeholder — waiting for rara-server crate");
        // Block until cancelled so the task stays alive in the JoinSet
        std::future::pending::<()>().await;
        Ok::<(), error::AppError>(())
    });

    // --- Research loop task ---
    if iterations > 0 {
        let bus = Arc::clone(&event_bus);
        let contract = contract_list.first().cloned().unwrap_or_default();
        let cfg = app_config::load();
        let cycle_delay = std::time::Duration::from_secs(cfg.research.cycle_delay_secs);
        tasks.spawn(async move {
            info!(iterations, contract = %contract, "research loop starting");
            let research_loop = build_research_loop(&trace_path, Arc::clone(&bus)).await?;
            run_research_loop(&research_loop, &bus, iterations, &contract, cycle_delay).await
        });
    }

    // --- Paper trading tasks (one per contract) ---
    for contract in &contract_list {
        let contract = contract.clone();
        let bus = Arc::clone(&event_bus);
        tasks.spawn(async move {
            info!(contract = %contract, "paper trading placeholder — wire up WS + aggregator + signal loop");
            // TODO: connect to exchange WS, run candle aggregator, feed
            // promoted strategies, and publish fills to event bus.
            let _ = bus;
            std::future::pending::<()>().await;
            Ok::<(), error::AppError>(())
        });
    }

    // --- Feedback consumer task ---
    {
        let bus = Arc::clone(&event_bus);
        let feedback_cfg = app_config::load().feedback.clone();
        tasks.spawn(async move {
            info!("feedback loop starting");
            let evaluator = build_strategy_evaluator(&feedback_cfg);
            let loop_config = rara_feedback::feedback_loop::FeedbackLoopConfig::builder()
                .eval_interval(std::time::Duration::from_secs(feedback_cfg.eval_interval_secs))
                .min_trades_between_evals(feedback_cfg.min_trades_between_evals)
                .build();
            rara_feedback::feedback_loop::run_feedback_loop(bus, evaluator, loop_config).await;
            Ok::<(), error::AppError>(())
        });
    }

    info!("daemon running — press Ctrl+C to shut down");

    // Wait for either a task to complete (crash) or Ctrl+C
    tokio::select! {
        result = tasks.join_next() => {
            match result {
                Some(Ok(Ok(()))) => {
                    info!("a task completed normally — initiating shutdown");
                }
                Some(Ok(Err(e))) => {
                    error!(error = %e, "a task failed — initiating shutdown");
                }
                Some(Err(join_err)) => {
                    error!(error = %join_err, "a task panicked — initiating shutdown");
                }
                None => {
                    info!("all tasks completed — shutting down");
                }
            }
        }
        _ = tokio::signal::ctrl_c() => {
            info!("received Ctrl+C — shutting down");
        }
    }

    // Cancel all remaining tasks
    tasks.abort_all();

    // Drain the JoinSet so all tasks have a chance to drop cleanly
    while tasks.join_next().await.is_some() {}

    info!("daemon stopped");
    Ok(())
}

/// Build a `ResearchLoop` from config, using the daemon's shared event bus
/// instead of creating a new one.
async fn build_research_loop(
    trace_path: &Path,
    event_bus: Arc<EventBus>,
) -> error::Result<ResearchLoop> {
    let template_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("strategies/template");
    let prompts_dir =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("crates/rara-research/src/prompts");

    let trace = Trace::open(trace_path).context(TraceSnafu)?;
    let compiler = StrategyCompiler::builder()
        .template_dir(template_dir)
        .build();
    let prompt_renderer =
        PromptRenderer::load_from_dir(&prompts_dir).context(PromptRendererSnafu)?;
    let prompt_renderer_for_loop =
        PromptRenderer::load_from_dir(&prompts_dir).context(PromptRendererSnafu)?;
    let cfg = app_config::load();
    let market_store = rara_market_data::store::MarketStore::connect(&cfg.database.url)
        .await
        .context(MarketStoreSnafu)?;

    let backtester: Arc<dyn crate::research::backtester::Backtester> =
        Arc::new(BarterBacktester::builder()
            .store(market_store)
            .initial_capital(dec!(10000))
            .fees_percent(dec!(0.1))
            .backtest_start(NaiveDate::from_ymd_opt(2020, 1, 1).expect("valid date"))
            .backtest_end(NaiveDate::from_ymd_opt(2030, 12, 31).expect("valid date"))
            .build());

    let cli_backend =
        CliBackend::from_agent_config(&cfg.agent).context(AgentBackendSnafu)?;
    let llm: Arc<dyn crate::infra::llm::LlmClient> =
        Arc::new(CliExecutor::new(cli_backend));

    let strategy_db_path = trace_path.join("strategy_db");
    let artifact_dir = paths::data_dir().join("artifacts");
    let strategy_store = StrategyStore::open_path(&strategy_db_path, &artifact_dir)
        .expect("failed to open strategy store");

    let strategy_manager: Arc<dyn crate::research::strategy_manager::StrategyManager> =
        Arc::new(WasmStrategyManager::builder()
            .store(strategy_store)
            .coder(StrategyCoder::new(Arc::clone(&llm)))
            .compiler(compiler)
            .executor(WasmExecutor::builder().build())
            .build());

    let feedback_gen = FeedbackGenerator::new(Arc::clone(&llm), prompt_renderer);
    let hypothesis_gen = HypothesisGenerator::new(llm);

    Ok(ResearchLoop::builder()
        .hypothesis_gen(hypothesis_gen)
        .strategy_manager(strategy_manager)
        .backtester(backtester)
        .feedback_gen(feedback_gen)
        .prompt_renderer(prompt_renderer_for_loop)
        .trace(trace)
        .event_bus(event_bus)
        .generated_dir(paths::strategies_generated_dir())
        .build())
}

/// Run research iterations in a continuous loop.
///
/// First executes `iterations` initial cycles, then waits for either a
/// [`FeedbackResearchRetrainRequested`](EventType::FeedbackResearchRetrainRequested)
/// event on the bus or a periodic timeout (`cycle_delay`), whichever comes
/// first, before starting the next batch of iterations.
async fn run_research_loop(
    research_loop: &ResearchLoop,
    event_bus: &EventBus,
    iterations: u32,
    contract: &str,
    cycle_delay: std::time::Duration,
) -> error::Result<()> {
    let mut rx = event_bus.subscribe();
    let mut cycle: u64 = 1;

    loop {
        info!(cycle, iterations, "research cycle starting");
        run_research_iterations(research_loop, iterations, contract, cycle_delay).await;

        info!(cycle, "research cycle complete — waiting for retrain signal or periodic timeout");

        // Wait for a retrain event or the periodic fallback timer
        loop {
            tokio::select! {
                result = rx.recv() => {
                    match result {
                        Ok(seq) => {
                            // Check if this sequence is a retrain event
                            if is_retrain_event(event_bus, seq) {
                                info!(seq, "received FeedbackResearchRetrainRequested — triggering new research cycle");
                                break;
                            }
                            // Not a retrain event, keep waiting
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                            warn!(skipped = n, "research event subscriber lagged — checking store for missed retrain events");
                            // After lagging, check if any retrain events were published
                            // recently; if so, trigger immediately
                            if has_pending_retrain_events(event_bus) {
                                info!("found pending retrain event after lag — triggering new research cycle");
                                break;
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                            info!("event bus closed — research loop exiting");
                            return Ok(());
                        }
                    }
                }
                () = tokio::time::sleep(cycle_delay) => {
                    info!(delay_secs = cycle_delay.as_secs(), "periodic research timeout — starting new cycle");
                    break;
                }
            }
        }

        cycle += 1;
    }
}

/// Run N research iterations with a configurable delay between cycles.
/// Errors from individual iterations are logged but do not abort the loop;
/// only the final summary is reported.
async fn run_research_iterations(
    research_loop: &ResearchLoop,
    iterations: u32,
    contract: &str,
    cycle_delay: std::time::Duration,
) {
    let mut accepted_count: u32 = 0;
    let mut rejected_count: u32 = 0;
    let mut error_count: u32 = 0;

    for i in 1..=iterations {
        info!(iteration = i, total = iterations, "research iteration starting");
        match research_loop.run_iteration(contract).await {
            Ok(ir) => {
                if ir.accepted {
                    accepted_count += 1;
                } else {
                    rejected_count += 1;
                }
                info!(
                    iteration = i,
                    total = iterations,
                    accepted = ir.accepted,
                    hypothesis = %ir.hypothesis.text,
                    "research iteration completed"
                );
            }
            Err(e) => {
                error_count += 1;
                error!(
                    iteration = i,
                    total = iterations,
                    error = %e,
                    "research iteration failed"
                );
            }
        }

        // Delay between cycles to avoid overwhelming external services
        if i < iterations {
            tokio::time::sleep(cycle_delay).await;
        }
    }

    info!(
        iterations,
        accepted = accepted_count,
        rejected = rejected_count,
        errors = error_count,
        "research iterations finished"
    );
}

/// Check whether the event at the given sequence number is a
/// [`FeedbackResearchRetrainRequested`](EventType::FeedbackResearchRetrainRequested).
fn is_retrain_event(event_bus: &EventBus, seq: u64) -> bool {
    event_bus
        .store()
        .get(seq)
        .ok()
        .flatten()
        .is_some_and(|e| e.event_type == EventType::FeedbackResearchRetrainRequested)
}

/// Scan the feedback topic for any recent retrain events that may have been
/// missed due to broadcast channel lag.
///
/// This is a best-effort fallback: it reads the last batch of feedback events
/// and returns `true` if any of them are retrain requests. The window is
/// intentionally small (last 50 events) to avoid expensive full-topic scans.
fn has_pending_retrain_events(event_bus: &EventBus) -> bool {
    // Read recent feedback events from the store; use offset 0 with a reasonable
    // limit since we only care about the presence of *any* retrain event.
    // In practice the consumer offset should be tracked, but for this fallback
    // a simple tail-scan is sufficient.
    event_bus
        .store()
        .read_topic("feedback", 0, 50)
        .unwrap_or_default()
        .iter()
        .any(|e| e.event_type == EventType::FeedbackResearchRetrainRequested)
}


/// Build a [`StrategyEvaluator`](rara_feedback::evaluator::StrategyEvaluator)
/// from the application's feedback configuration.
fn build_strategy_evaluator(
    cfg: &crate::app_config::FeedbackConfig,
) -> rara_feedback::evaluator::StrategyEvaluator {
    // Config stores drawdown as a percentage (e.g. 20.0 for 20%); the evaluator
    // compares against absolute Decimal values from the accumulator, so we
    // convert percentage → fraction (20.0 → 0.20).
    let demote_drawdown = rust_decimal::Decimal::try_from(cfg.max_drawdown_for_retirement / 100.0)
        .expect("drawdown config must be a valid decimal");

    rara_feedback::evaluator::StrategyEvaluator::new(
        cfg.min_sharpe_for_promotion,
        demote_drawdown,
        cfg.min_trades,
    )
}
