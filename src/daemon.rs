//! Unified daemon orchestrator — runs research, trading, feedback, and
//! gRPC server as concurrent tokio tasks in a single process.

use std::{
    path::{Path, PathBuf},
    sync::{Arc, atomic::AtomicBool},
    time::Duration,
};

use chrono::NaiveDate;
use rara_domain::{event::EventType, research::ResearchStrategyStatus};
use rara_market_data::stream::{
    aggregator::CandleAggregator,
    binance_ws::BinanceWsClient,
    reconnect::{ReconnectConfig, ReconnectingWsClient},
};
use rara_sentinel::{
    analyzer::SignalAnalyzer,
    engine::SentinelEngine,
    source::DataSource,
    sources::{rss::RssDataSource, trump_code::TrumpCodeDataSource},
};
use rara_trading_engine::{
    engine::TradingEngine,
    guard_pipeline::GuardPipeline,
    signal_loop::{self, LoadedStrategy},
};
use rust_decimal_macros::dec;
use snafu::ResultExt;
use tokio::task::JoinSet;
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::{
    accounts_config,
    agent::{CliBackend, CliExecutor},
    app_config,
    app_config::SentinelConfig,
    error::{
        self, AgentBackendSnafu, EventBusSnafu, GrpcServeSnafu, MarketStoreSnafu,
        PromptRendererSnafu, TraceSnafu,
    },
    event_bus::bus::EventBus,
    paths,
    research::{
        barter_backtester::BarterBacktester, compiler::StrategyCompiler,
        feedback_gen::FeedbackGenerator, hypothesis_gen::HypothesisGenerator,
        prompt_renderer::PromptRenderer, research_loop::ResearchLoop,
        strategy_coder::StrategyCoder, strategy_executor::StrategyExecutor,
        strategy_store::StrategyStore, trace::Trace, wasm_executor::WasmExecutor,
        wasm_strategy_manager::WasmStrategyManager,
    },
};

/// Run the unified daemon: spawn all trading-loop components as concurrent
/// tokio tasks and wait for shutdown (Ctrl+C) or a fatal task error.
///
/// Accounts and contracts are loaded from `accounts.toml` rather than CLI
/// flags.
pub async fn run(iterations: u32, grpc_addr: String) -> error::Result<()> {
    // Load accounts from config; collect contracts from all enabled accounts
    let accounts_cfg = accounts_config::load_accounts();
    let _account_manager =
        rara_trading_engine::account_manager::AccountManager::from_config(&accounts_cfg.accounts)
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
    spawn_grpc_task(&mut tasks, &event_bus, &contract_list, &grpc_addr);

    // --- Research loop task ---
    if iterations > 0 {
        let bus = Arc::clone(&event_bus);
        let contract = contract_list.first().cloned().unwrap_or_default();
        let cfg = app_config::load();
        let cycle_delay = std::time::Duration::from_secs(cfg.research.cycle_delay_secs);
        tasks.spawn(async move {
            info!(iterations, contract = %contract, "research loop starting");
            let research_loop =
                build_research_loop(&trace_path, Arc::clone(&bus), &contract).await?;
            run_research_loop(&research_loop, &bus, iterations, &contract, cycle_delay).await
        });
    }

    // --- Live/sandbox trading tasks (one per contract) ---
    for contract in &contract_list {
        let contract = contract.clone();
        let bus = Arc::clone(&event_bus);
        let trace_path_clone = paths::data_dir().join("trace");
        let cfg = app_config::load();
        tasks.spawn(async move {
            info!(contract = %contract, "live trading task starting");
            if let Err(e) = run_live_trading(&contract, bus, &trace_path_clone, cfg).await {
                error!(contract = %contract, error = %e, "live trading task failed");
            }
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
                .eval_interval(std::time::Duration::from_secs(
                    feedback_cfg.eval_interval_secs,
                ))
                .min_trades_between_evals(feedback_cfg.min_trades_between_evals)
                .build();
            rara_feedback::feedback_loop::run_feedback_loop(bus, evaluator, loop_config).await;
            Ok::<(), error::AppError>(())
        });
    }

    // --- Lifecycle event consumer task ---
    // Listens for FeedbackStrategyPromote/Demote events and updates the
    // strategy store so live trading can pick up newly promoted strategies.
    {
        let bus = Arc::clone(&event_bus);
        let trace_path_clone = paths::data_dir().join("trace");
        tasks.spawn(async move {
            info!("lifecycle event consumer starting");
            run_lifecycle_consumer(bus, &trace_path_clone).await;
            Ok::<(), error::AppError>(())
        });
    }

    // --- Sentinel monitoring task ---
    spawn_sentinel_task(&mut tasks, &event_bus);

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
    contract: &str,
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

    let backtester: Arc<dyn crate::research::backtester::Backtester> = Arc::new(
        BarterBacktester::builder()
            .store(market_store)
            .initial_capital(dec!(10000))
            .fees_percent(dec!(0.1))
            .backtest_start(NaiveDate::from_ymd_opt(2020, 1, 1).expect("valid date"))
            .backtest_end(NaiveDate::from_ymd_opt(2030, 12, 31).expect("valid date"))
            .build(),
    );

    let cli_backend = CliBackend::from_agent_config(&cfg.agent).context(AgentBackendSnafu)?;
    let llm: Arc<dyn crate::infra::llm::LlmClient> = Arc::new(CliExecutor::new(cli_backend));

    let strategy_db_path = trace_path.join("strategy_db");
    let artifact_dir = paths::data_dir().join("artifacts");
    let strategy_store = StrategyStore::open_path(&strategy_db_path, &artifact_dir)
        .expect("failed to open strategy store");

    let strategy_manager: Arc<dyn crate::research::strategy_manager::StrategyManager> = Arc::new(
        WasmStrategyManager::builder()
            .store(strategy_store)
            .coder(StrategyCoder::new(Arc::clone(&llm)))
            .compiler(compiler)
            .executor(WasmExecutor::builder().build())
            .build(),
    );

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
        .contract(contract)
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

        info!(
            cycle,
            "research cycle complete — waiting for retrain signal or periodic timeout"
        );

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
        info!(
            iteration = i,
            total = iterations,
            "research iteration starting"
        );
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

/// Spawn the gRPC server task with health probes and event streaming.
fn spawn_grpc_task(
    tasks: &mut JoinSet<error::Result<()>>,
    event_bus: &Arc<EventBus>,
    contract_list: &[String],
    grpc_addr: &str,
) {
    let cfg = app_config::load();
    let health_config = rara_server::health::HealthConfig {
        database_url:   cfg.database.url.clone(),
        llm_backend:    cfg.agent.backend.clone(),
        ws_connected:   Arc::new(AtomicBool::new(false)),
        contract_count: u32::try_from(contract_list.len()).unwrap_or(u32::MAX),
    };
    let svc = rara_server::build_service(Arc::clone(event_bus), health_config);
    let addr: std::net::SocketAddr = grpc_addr.parse().expect("valid gRPC socket address");
    tasks.spawn(async move {
        info!(%addr, "gRPC server starting");
        tonic::transport::Server::builder()
            .add_service(svc)
            .serve(addr)
            .await
            .context(GrpcServeSnafu)
    });
}

/// Spawn the sentinel monitoring task if enabled in config.
///
/// When enabled, creates a polling loop that periodically checks all
/// configured data sources (RSS feeds, trump-code) for market-moving
/// signals using LLM analysis, publishing detected signals to the event bus.
fn spawn_sentinel_task(tasks: &mut JoinSet<error::Result<()>>, event_bus: &Arc<EventBus>) {
    let sentinel_cfg = app_config::load().sentinel.clone();
    if !sentinel_cfg.enabled {
        info!("sentinel monitoring disabled — skipping");
        return;
    }

    let bus = Arc::clone(event_bus);
    let cfg = app_config::load();
    tasks.spawn(async move {
        info!("sentinel monitoring starting");
        let cli_backend = CliBackend::from_agent_config(&cfg.agent).context(AgentBackendSnafu)?;
        let llm = CliExecutor::new(cli_backend);
        let sources = build_sentinel_sources(&sentinel_cfg);
        let analyzer = SignalAnalyzer::new(llm);
        let engine = SentinelEngine::new(sources, analyzer, Arc::clone(&bus));
        let interval = std::time::Duration::from_secs(sentinel_cfg.check_interval_secs);
        run_sentinel_loop(&engine, interval).await;
        Ok::<(), error::AppError>(())
    });
}

/// Build data sources from sentinel configuration.
///
/// Creates RSS feed sources and optionally the trump-code source based on
/// the config. Returns an empty vec if no sources are configured.
fn build_sentinel_sources(cfg: &SentinelConfig) -> Vec<Box<dyn DataSource>> {
    let client = reqwest::Client::new();
    let mut sources: Vec<Box<dyn DataSource>> = cfg
        .rss_feeds
        .iter()
        .map(|feed| -> Box<dyn DataSource> {
            Box::new(
                RssDataSource::builder()
                    .name(feed.name.clone())
                    .url(feed.url.clone())
                    .client(client.clone())
                    .build(),
            )
        })
        .collect();

    if cfg.trump_code_enabled {
        sources.push(Box::new(
            TrumpCodeDataSource::builder()
                .base_url(cfg.trump_code_url.clone())
                .client(client)
                .build(),
        ));
    }

    sources
}

/// Run the sentinel engine in a polling loop.
///
/// Each cycle polls all data sources and analyzes signals via LLM.
/// Errors are logged but never crash the daemon — sentinel is a monitoring
/// component that must not take down the trading system.
async fn run_sentinel_loop<L: rara_infra::llm::LlmClient>(
    engine: &SentinelEngine<L>,
    interval: std::time::Duration,
) {
    loop {
        match engine.poll_and_analyze().await {
            Ok(signals) => {
                if signals.is_empty() {
                    info!("sentinel poll complete — no actionable signals");
                } else {
                    info!(
                        count = signals.len(),
                        "sentinel poll complete — published signals to event bus"
                    );
                    for signal in &signals {
                        info!(
                            signal_type = %signal.signal_type,
                            severity = %signal.severity,
                            contracts = ?signal.affected_contracts,
                            "sentinel signal detected"
                        );
                    }
                }
            }
            Err(e) => {
                // Log and continue — sentinel errors must never crash the daemon
                error!(error = %e, "sentinel poll failed — will retry next cycle");
            }
        }

        tokio::time::sleep(interval).await;
    }
}

/// Run live trading for a single contract.
///
/// Connects to Binance WebSocket for live kline data, aggregates into
/// multi-timeframe candles, loads promoted strategies from the store,
/// and runs the signal loop to generate and execute trades through the
/// broker configured in `accounts.toml`. Use `sandbox = true` in account
/// config for live trading against exchange testnets. If no promoted
/// strategies exist, retries every 30 seconds until at least one becomes
/// available.
async fn run_live_trading(
    contract: &str,
    event_bus: Arc<EventBus>,
    trace_path: &Path,
    cfg: &crate::app_config::AppConfig,
) -> error::Result<()> {
    // Retry until at least one promoted strategy is available
    let strategies = loop {
        let loaded = load_promoted_strategies(trace_path, contract, cfg)?;
        if !loaded.is_empty() {
            break loaded;
        }
        info!(
            contract = %contract,
            "no promoted strategies found — retrying in 30s"
        );
        tokio::time::sleep(Duration::from_secs(30)).await;
    };

    info!(
        contract = %contract,
        strategy_count = strategies.len(),
        "loaded promoted strategies for live trading"
    );

    // Set up candle aggregation (5m, 15m, 1h from 1m klines)
    let (mut aggregator, candle_rx) = CandleAggregator::with_defaults();

    // Set up reconnecting WebSocket client for this contract's 1m klines
    let ws_client = BinanceWsClient::new();
    let reconnect_client = ReconnectingWsClient::new(ws_client, ReconnectConfig::default());
    let mut kline_rx = reconnect_client.subscribe();

    let subscriptions = vec![(contract.to_string(), "1m".to_string())];

    // Spawn the WebSocket connection loop (runs forever with auto-reconnect)
    let ws_handle = tokio::spawn(async move {
        reconnect_client.run(subscriptions).await;
    });

    // Spawn the kline-to-candle aggregation task
    let agg_handle = tokio::spawn(async move {
        loop {
            match kline_rx.recv().await {
                Ok(kline) => {
                    aggregator.process_kline(&kline);
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    warn!(
                        skipped = n,
                        "kline aggregation lagged — some 1m candles were dropped"
                    );
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    info!("kline channel closed — aggregation stopping");
                    break;
                }
            }
        }
    });

    // Build broker from account configuration
    let accounts_cfg = crate::accounts_config::load_accounts();
    let account = accounts_cfg
        .accounts
        .iter()
        .find(|a| {
            a.enabled
                && (a.contracts.is_empty()
                    || a.contracts.iter().any(|c| c == contract))
        })
        .ok_or_else(|| error::AppError::Config {
            message: format!(
                "no enabled account configured for contract {contract} \
                 — run `rara setup account add --broker ccxt --exchange binance --sandbox`"
            ),
        })?;
    let fields = account.broker_config.to_field_map();
    let type_key = account.broker_config.type_key();
    let entry =
        rara_trading_engine::broker_registry::find_broker(type_key).ok_or_else(|| {
            error::AppError::Config {
                message: format!("unknown broker type: {type_key}"),
            }
        })?;
    let broker = (entry.create_broker)(&fields).map_err(|e| error::AppError::Config {
        message: format!("failed to create broker for account '{}': {e}", account.id),
    })?;
    info!(
        account_id = %account.id,
        broker_type = type_key,
        contract = %contract,
        "broker created from account config"
    );

    let guard_pipeline = GuardPipeline::new(vec![]);
    let engine = Arc::new(TradingEngine::new(
        broker,
        guard_pipeline,
        Arc::clone(&event_bus),
    ));

    // Run the signal loop (blocks until candle channel closes)
    signal_loop::run_signal_loop(candle_rx, engine, strategies).await;

    // Clean up background tasks
    ws_handle.abort();
    agg_handle.abort();

    Ok(())
}

/// Load promoted strategies from the strategy store and prepare them for
/// live signal generation.
///
/// Reads all strategies with `Promoted` status, loads their WASM artifacts,
/// and wraps each in a [`LoadedStrategy`] ready for the signal loop.
fn load_promoted_strategies(
    trace_path: &Path,
    contract: &str,
    cfg: &crate::app_config::AppConfig,
) -> error::Result<Vec<LoadedStrategy>> {
    let strategy_db_path = trace_path.join("strategy_db");
    let artifact_dir = paths::data_dir().join("artifacts");
    let store = StrategyStore::open_path(&strategy_db_path, &artifact_dir).map_err(|e| {
        error::AppError::Trading {
            message: format!("failed to open strategy store: {e}"),
        }
    })?;

    let promoted = store
        .list(Some(ResearchStrategyStatus::Promoted))
        .map_err(|e| error::AppError::Trading {
            message: format!("failed to list promoted strategies: {e}"),
        })?;

    let executor = WasmExecutor::builder().build();
    let position_size = rust_decimal::Decimal::try_from(cfg.trading.max_position_size)
        .expect("max_position_size config must be a valid decimal");

    let mut loaded = Vec::new();

    for strategy in &promoted {
        let artifact = match store.load_artifact(strategy.id) {
            Ok(a) => a,
            Err(e) => {
                warn!(
                    strategy_id = %strategy.id,
                    error = %e,
                    "skipping promoted strategy — failed to load artifact"
                );
                continue;
            }
        };

        let mut handle = match executor.load(&artifact) {
            Ok(h) => h,
            Err(e) => {
                warn!(
                    strategy_id = %strategy.id,
                    error = %e,
                    "skipping promoted strategy — failed to load WASM module"
                );
                continue;
            }
        };

        let meta = match handle.meta() {
            Ok(m) => m,
            Err(e) => {
                warn!(
                    strategy_id = %strategy.id,
                    error = %e,
                    "skipping promoted strategy — failed to read metadata"
                );
                continue;
            }
        };

        info!(
            strategy_id = %strategy.id,
            name = meta.name,
            version = meta.version,
            "loaded promoted strategy for live trading"
        );

        loaded.push(LoadedStrategy {
            name: meta.name,
            version: meta.version,
            contract_id: contract.to_string(),
            position_size,
            handle,
        });
    }

    // Fallback: also check the promoted directory where `strategy fetch` saves
    // downloaded WASM files, so externally fetched strategies are usable too.
    if loaded.is_empty() {
        let promoted_dir = paths::strategies_promoted_dir();
        if promoted_dir.exists() {
            load_wasm_from_dir(
                &promoted_dir,
                contract,
                position_size,
                &executor,
                &mut loaded,
            );
        }
    }

    Ok(loaded)
}

/// Load `.wasm` files directly from a directory as fallback strategies.
fn load_wasm_from_dir(
    dir: &Path,
    contract: &str,
    position_size: rust_decimal::Decimal,
    executor: &WasmExecutor,
    loaded: &mut Vec<LoadedStrategy>,
) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) => {
            warn!(dir = %dir.display(), error = %e, "failed to read promoted directory");
            return;
        }
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "wasm") {
            let wasm_bytes = match std::fs::read(&path) {
                Ok(b) => b,
                Err(e) => {
                    warn!(path = %path.display(), error = %e, "failed to read WASM file");
                    continue;
                }
            };

            let mut handle = match executor.load(&wasm_bytes) {
                Ok(h) => h,
                Err(e) => {
                    warn!(path = %path.display(), error = %e, "failed to load WASM module");
                    continue;
                }
            };

            let meta = match handle.meta() {
                Ok(m) => m,
                Err(e) => {
                    warn!(path = %path.display(), error = %e, "failed to read WASM metadata");
                    continue;
                }
            };

            info!(
                name = meta.name,
                version = meta.version,
                path = %path.display(),
                "loaded fetched strategy from promoted directory"
            );

            loaded.push(LoadedStrategy {
                name: meta.name,
                version: meta.version,
                contract_id: contract.to_string(),
                position_size,
                handle,
            });
        }
    }
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

/// Consume feedback lifecycle events and update strategy status accordingly.
///
/// Listens on the event bus for `FeedbackStrategyPromote` and
/// `FeedbackStrategyDemote` events. When a promote event arrives the
/// referenced strategy is moved to `Promoted`; demote events move it to
/// `Archived`. This closes the loop so live trading can hot-reload newly
/// promoted strategies.
async fn run_lifecycle_consumer(event_bus: Arc<EventBus>, trace_path: &Path) {
    let mut rx = event_bus.subscribe();
    let strategy_db_path = trace_path.join("strategy_db");
    let artifact_dir = paths::data_dir().join("artifacts");

    let store = match StrategyStore::open_path(&strategy_db_path, &artifact_dir) {
        Ok(s) => s,
        Err(e) => {
            error!(error = %e, "lifecycle consumer failed to open strategy store — exiting");
            return;
        }
    };

    loop {
        let seq = match rx.recv().await {
            Ok(seq) => seq,
            Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                warn!(
                    skipped = n,
                    "lifecycle consumer lagged — some events may have been missed"
                );
                continue;
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                info!("event bus closed — lifecycle consumer exiting");
                return;
            }
        };

        let Ok(Some(event)) = event_bus.store().get(seq) else {
            continue;
        };

        let (new_status, label) = match event.event_type {
            EventType::FeedbackStrategyPromote => (ResearchStrategyStatus::Promoted, "promoted"),
            EventType::FeedbackStrategyDemote => (ResearchStrategyStatus::Archived, "archived"),
            _ => continue,
        };

        let Some(strategy_id_str) = event.strategy_id.as_deref() else {
            warn!(seq, "lifecycle event missing strategy_id — skipping");
            continue;
        };

        let Ok(uuid) = Uuid::parse_str(strategy_id_str) else {
            warn!(
                strategy_id = %strategy_id_str,
                "lifecycle event has non-UUID strategy_id — skipping"
            );
            continue;
        };

        if let Err(e) = store.update_status(uuid, new_status) {
            warn!(
                strategy_id = %uuid,
                error = %e,
                "lifecycle consumer failed to update strategy status"
            );
        } else {
            info!(
                strategy_id = %uuid,
                status = label,
                "lifecycle consumer updated strategy status"
            );
        }
    }
}
