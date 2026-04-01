//! Unified daemon orchestrator — runs research, paper trading, feedback, and
//! gRPC server as concurrent tokio tasks in a single process.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::NaiveDate;
use rust_decimal_macros::dec;
use snafu::ResultExt;
use tokio::task::JoinSet;
use tracing::{error, info};

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
            let research_loop = build_research_loop(&trace_path, bus).await?;
            run_research_iterations(&research_loop, iterations, &contract, cycle_delay).await
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
        tasks.spawn(async move {
            info!("feedback consumer placeholder — wire up event-driven feedback loop");
            // TODO: subscribe to event bus, consume paper-trading fills,
            // generate feedback, and publish results.
            let _ = bus;
            std::future::pending::<()>().await;
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

/// Run N research iterations with a configurable delay between cycles.
/// Errors from individual iterations are logged but do not abort the loop;
/// only the final summary is reported.
async fn run_research_iterations(
    research_loop: &ResearchLoop,
    iterations: u32,
    contract: &str,
    cycle_delay: std::time::Duration,
) -> error::Result<()> {
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
        "research loop finished"
    );

    Ok(())
}
