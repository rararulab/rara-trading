#![allow(clippy::result_large_err)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::NaiveDate;
use clap::Parser;
use rust_decimal_macros::dec;
use serde::Serialize;
use snafu::ResultExt;

use rara_trading::agent::{CliBackend, CliExecutor};
use rara_trading::app_config;
use rara_trading::cli::{Cli, Command, ConfigAction, DataAction, FeedbackAction, PaperAction, ResearchAction};
use rara_trading::validation;
use rara_trading::error::{
    self, AgentBackendSnafu, AgentExecutionSnafu, ConfigSnafu, DataFetchSnafu, EventBusSnafu,
    GrpcServeSnafu, IoSnafu, MarketStoreSnafu, PromoterSnafu, PromptRendererSnafu, TraceSnafu,
    TuiSnafu,
};
use rara_trading::event_bus::bus::EventBus;
use rara_trading::logging::{self, LoggingConfig};
use rara_trading::paths;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// CLI response types — compile-time typed JSON output
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct ErrorResponse {
    ok: bool,
    error: String,
}

#[derive(Serialize)]
struct ConfigSetResponse<'a> {
    ok: bool,
    action: &'static str,
    key: &'a str,
    value: &'a str,
}

#[derive(Serialize)]
struct ConfigGetResponse<'a> {
    ok: bool,
    action: &'static str,
    key: &'a str,
    value: &'a str,
}

#[derive(Serialize)]
struct ConfigListResponse {
    ok: bool,
    action: &'static str,
    entries: serde_json::Map<String, serde_json::Value>,
}

#[derive(Serialize)]
struct ConfigInitResponse<'a> {
    ok: bool,
    action: &'static str,
    path: &'a str,
}

#[derive(Serialize)]
struct ValidateResponse {
    ok: bool,
    action: &'static str,
    errors: Vec<String>,
}

#[derive(Serialize)]
struct HelloResponse<'a> {
    ok: bool,
    action: &'static str,
    greeting: &'a str,
}

#[derive(Serialize)]
struct AgentResponse<'a> {
    ok: bool,
    action: &'static str,
    exit_code: Option<i32>,
    timed_out: bool,
    output: &'a str,
}

#[derive(Serialize)]
struct IterationResponse<'a> {
    iteration: u32,
    accepted: bool,
    hypothesis: &'a str,
}

#[derive(Serialize)]
struct ResearchRunResponse {
    ok: bool,
    action: &'static str,
    iterations: u32,
    accepted: u32,
    rejected: u32,
    errors: u32,
}

#[derive(Serialize)]
struct DataFetchResponse<'a> {
    ok: bool,
    action: &'static str,
    source: &'a str,
    symbol: &'a str,
    candles: usize,
}

#[derive(Serialize)]
struct DataInfoResponse {
    ok: bool,
    action: &'static str,
    instruments: Vec<rara_market_data::store::candle::CandleCoverage>,
}

#[derive(Serialize)]
struct ExperimentListItem {
    index: u64,
    experiment_id: String,
    hypothesis: String,
    decision: &'static str,
    sharpe: Option<f64>,
}

#[derive(Serialize)]
struct ResearchListResponse {
    ok: bool,
    action: &'static str,
    experiments: Vec<ExperimentListItem>,
}

#[derive(Serialize)]
struct HypothesisDetail {
    id: String,
    text: String,
    reason: String,
    observation: String,
    knowledge: String,
    parent: Option<String>,
}

#[derive(Serialize)]
struct FeedbackDetail {
    experiment_id: String,
    decision: bool,
    reason: String,
    observations: String,
    hypothesis_evaluation: String,
    new_hypothesis: Option<String>,
    code_change_summary: String,
}

#[derive(Serialize)]
struct BacktestDetail {
    pnl: String,
    sharpe_ratio: f64,
    max_drawdown: String,
    win_rate: f64,
    trade_count: u32,
}

#[derive(Serialize)]
struct ExperimentDetail {
    id: String,
    hypothesis_id: String,
    status: String,
    strategy_code: String,
    backtest_result: Option<BacktestDetail>,
}

#[derive(Serialize)]
struct ResearchShowResponse {
    ok: bool,
    action: &'static str,
    experiment: ExperimentDetail,
    hypothesis: Option<HypothesisDetail>,
    feedbacks: Vec<FeedbackDetail>,
}

#[derive(Serialize)]
struct PromotedItem {
    experiment_id: String,
    hypothesis_id: String,
    wasm_path: String,
    source_path: Option<String>,
    meta: PromotedMeta,
}

#[derive(Serialize)]
struct PromotedMeta {
    name: String,
    version: u32,
    api_version: u32,
    description: String,
}

#[derive(Serialize)]
struct ResearchPromotedResponse {
    ok: bool,
    action: &'static str,
    strategies: Vec<PromotedItem>,
}

#[derive(Serialize)]
struct EvaluationEntry {
    timestamp: String,
    strategy_id: String,
    decision: String,
    reason: String,
    sharpe_ratio: f64,
    win_rate: f64,
    trade_count: u64,
    pnl: String,
    max_drawdown: String,
}

#[derive(Serialize)]
struct FeedbackReportResponse {
    ok: bool,
    action: &'static str,
    evaluations: Vec<EvaluationEntry>,
}

/// Per-strategy aggregated status from event bus trading events.
#[derive(Serialize)]
struct StrategyStatus {
    strategy: String,
    trades: usize,
    filled: usize,
    rejected: usize,
}

/// Response payload for `paper status`.
#[derive(Serialize)]
struct PaperStatusResponse {
    ok: bool,
    action: &'static str,
    strategies: Vec<StrategyStatus>,
    total_trades: usize,
}

use rara_trading::research::barter_backtester::BarterBacktester;
use rara_trading::research::compiler::StrategyCompiler;
use rara_trading::research::strategy_executor::StrategyExecutor;
use rara_trading::research::wasm_executor::WasmExecutor;
use rara_trading::research::feedback_gen::FeedbackGenerator;
use rara_trading::research::hypothesis_gen::HypothesisGenerator;
use rara_trading::research::prompt_renderer::PromptRenderer;
use rara_trading::research::research_loop::ResearchLoop;
use rara_trading::research::strategy_coder::StrategyCoder;
use rara_trading::research::strategy_promoter::PromotedStrategy;
use rara_trading::research::strategy_store::StrategyStore;
use rara_trading::research::trace::Trace;
use rara_trading::research::wasm_strategy_manager::WasmStrategyManager;

#[tokio::main]
async fn main() {
    let logging_config = LoggingConfig::default();
    // Hold the guard so file logs flush on shutdown
    let _log_guard = logging::init_logging(&logging_config);

    if let Err(e) = run().await {
        tracing::error!(error = %e, "application error");
        println!(
            "{}",
            serde_json::to_string(&ErrorResponse {
                ok: false,
                error: e.to_string(),
            })
            .expect("ErrorResponse must serialize")
        );
        std::process::exit(1);
    }
}

#[allow(clippy::too_many_lines)]
async fn run() -> error::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Config { action } => match action {
            ConfigAction::Init { force } => {
                let path = paths::config_file();
                if path.exists() && !force {
                    return ConfigSnafu {
                        message: format!(
                            "config file already exists at {}. Use --force to overwrite.",
                            path.display()
                        ),
                    }
                    .fail();
                }
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent).context(IoSnafu)?;
                }
                let template = app_config::generate_template();
                std::fs::write(&path, &template).context(IoSnafu)?;
                let path_str = path.display().to_string();
                eprintln!("config template written to {path_str}");
                println!(
                    "{}",
                    serde_json::to_string(&ConfigInitResponse {
                        ok: true,
                        action: "config_init",
                        path: &path_str,
                    })
                    .expect("ConfigInitResponse must serialize")
                );
            }
            ConfigAction::Set { key, value } => {
                let mut cfg = app_config::load().clone();
                set_config_field(&mut cfg, &key, &value)?;
                app_config::save(&cfg).context(IoSnafu)?;
                tracing::info!(key = %key, value = %value, "config updated");
                println!(
                    "{}",
                    serde_json::to_string(&ConfigSetResponse {
                        ok: true,
                        action: "config_set",
                        key: &key,
                        value: &value,
                    })
                    .expect("ConfigSetResponse must serialize")
                );
            }
            ConfigAction::Get { key } => {
                let cfg = app_config::load();
                let value = get_config_field(cfg, &key)?;
                let display_value = value.as_deref().unwrap_or("(not set)");
                println!(
                    "{}",
                    serde_json::to_string(&ConfigGetResponse {
                        ok: true,
                        action: "config_get",
                        key: &key,
                        value: display_value,
                    })
                    .expect("ConfigGetResponse must serialize")
                );
            }
            ConfigAction::List => {
                let cfg = app_config::load();
                let entries = config_as_map(cfg);
                let map: serde_json::Map<String, serde_json::Value> = entries
                    .into_iter()
                    .map(|(k, v)| (k, serde_json::Value::String(v)))
                    .collect();
                println!(
                    "{}",
                    serde_json::to_string(&ConfigListResponse {
                        ok: true,
                        action: "config_list",
                        entries: map,
                    })
                    .expect("ConfigListResponse must serialize")
                );
            }
        },
        Command::Hello { name } => {
            let greeting = format!("Hello, {name}!");
            tracing::info!(name = %name, "hello command executed");
            println!(
                "{}",
                serde_json::to_string(&HelloResponse {
                    ok: true,
                    action: "hello",
                    greeting: &greeting,
                })
                .expect("HelloResponse must serialize")
            );
        }
        Command::Research { action } => {
            run_research(action).await?;
        }
        Command::Data { action } => {
            run_data(action).await?;
        }
        Command::Run {
            contracts,
            iterations,
            grpc_addr,
        } => {
            rara_trading::daemon::run(contracts, iterations, grpc_addr).await?;
        }
        Command::Validate => {
            let cfg = app_config::load();
            let errors = validation::validate_startup(cfg).await;
            let error_strings: Vec<String> = errors.iter().map(ToString::to_string).collect();
            if errors.is_empty() {
                eprintln!("All checks passed");
                println!(
                    "{}",
                    serde_json::to_string(&ValidateResponse {
                        ok: true,
                        action: "validate",
                        errors: vec![],
                    })
                    .expect("ValidateResponse must serialize")
                );
            } else {
                for e in &errors {
                    eprintln!("FAIL: {e}");
                }
                println!(
                    "{}",
                    serde_json::to_string(&ValidateResponse {
                        ok: false,
                        action: "validate",
                        errors: error_strings,
                    })
                    .expect("ValidateResponse must serialize")
                );
                std::process::exit(1);
            }
        }
        Command::Serve { port } => {
            run_serve(port).await?;
        }
        Command::Tui { server } => {
            rara_tui::event_loop::run(&server).await.context(TuiSnafu)?;
        }
        Command::Feedback { action } => {
            run_feedback(action)?;
        }
        Command::Paper { action } => {
            run_paper(action).await?;
        }
        Command::Agent { prompt, backend } => {
            let cfg = app_config::load();
            let mut agent_cfg = cfg.agent.clone();
            if let Some(b) = backend {
                agent_cfg.backend = b;
            }

            let cli_backend =
                CliBackend::from_agent_config(&agent_cfg).context(AgentBackendSnafu)?;
            let executor = CliExecutor::new(cli_backend);

            let timeout = if agent_cfg.idle_timeout_secs > 0 {
                Some(std::time::Duration::from_secs(u64::from(
                    agent_cfg.idle_timeout_secs,
                )))
            } else {
                None
            };

            let result = executor
                .execute(&prompt, std::io::stderr(), timeout, false)
                .await
                .context(AgentExecutionSnafu)?;

            if !result.stderr.is_empty() {
                eprint!("{}", result.stderr);
            }

            println!(
                "{}",
                serde_json::to_string(&AgentResponse {
                    ok: result.success,
                    action: "agent_run",
                    exit_code: result.exit_code,
                    timed_out: result.timed_out,
                    output: &result.output,
                })
                .expect("AgentResponse must serialize")
            );
        }
    }

    Ok(())
}

/// Set a config field by dotted key path.
fn set_config_field(cfg: &mut app_config::AppConfig, key: &str, value: &str) -> error::Result<()> {
    let parse_err = |key: &str, value: &str| error::AppError::Config {
        message: format!("invalid value for {key}: {value}"),
    };
    match key {
        // agent
        "agent.backend" => cfg.agent.backend = value.to_string(),
        "agent.command" => cfg.agent.command = Some(value.to_string()),
        "agent.idle_timeout_secs" => {
            cfg.agent.idle_timeout_secs = value.parse().map_err(|_| parse_err(key, value))?;
        }
        // database
        "database.url" => cfg.database.url = value.to_string(),
        // trading
        "trading.broker" => cfg.trading.broker = value.to_string(),
        "trading.max_position_size" => {
            cfg.trading.max_position_size = value.parse().map_err(|_| parse_err(key, value))?;
        }
        "trading.max_drawdown_pct" => {
            cfg.trading.max_drawdown_pct = value.parse().map_err(|_| parse_err(key, value))?;
        }
        "trading.max_concurrent_positions" => {
            cfg.trading.max_concurrent_positions =
                value.parse().map_err(|_| parse_err(key, value))?;
        }
        // research
        "research.iterations" => {
            cfg.research.iterations = value.parse().map_err(|_| parse_err(key, value))?;
        }
        "research.max_compile_retries" => {
            cfg.research.max_compile_retries = value.parse().map_err(|_| parse_err(key, value))?;
        }
        // feedback
        "feedback.min_sharpe_for_promotion" => {
            cfg.feedback.min_sharpe_for_promotion =
                value.parse().map_err(|_| parse_err(key, value))?;
        }
        "feedback.min_win_rate" => {
            cfg.feedback.min_win_rate = value.parse().map_err(|_| parse_err(key, value))?;
        }
        "feedback.min_trades" => {
            cfg.feedback.min_trades = value.parse().map_err(|_| parse_err(key, value))?;
        }
        "feedback.max_drawdown_for_retirement" => {
            cfg.feedback.max_drawdown_for_retirement =
                value.parse().map_err(|_| parse_err(key, value))?;
        }
        // sentinel
        "sentinel.enabled" => {
            cfg.sentinel.enabled = value.parse().map_err(|_| parse_err(key, value))?;
        }
        "sentinel.check_interval_secs" => {
            cfg.sentinel.check_interval_secs = value.parse().map_err(|_| parse_err(key, value))?;
        }
        // server
        "server.listen_addr" => cfg.server.listen_addr = value.to_string(),
        "server.port" => {
            cfg.server.port = value.parse().map_err(|_| parse_err(key, value))?;
        }
        _ => return ConfigSnafu { message: format!("unknown config key: {key}") }.fail(),
    }
    Ok(())
}

/// Get a config field by dotted key path.
fn get_config_field(cfg: &app_config::AppConfig, key: &str) -> error::Result<Option<String>> {
    match key {
        // agent
        "agent.backend" => Ok(Some(cfg.agent.backend.clone())),
        "agent.command" => Ok(cfg.agent.command.clone()),
        "agent.idle_timeout_secs" => Ok(Some(cfg.agent.idle_timeout_secs.to_string())),
        // database
        "database.url" => Ok(Some(cfg.database.url.clone())),
        // trading
        "trading.broker" => Ok(Some(cfg.trading.broker.clone())),
        "trading.contracts" => Ok(Some(cfg.trading.contracts.join(","))),
        "trading.max_position_size" => Ok(Some(cfg.trading.max_position_size.to_string())),
        "trading.max_drawdown_pct" => Ok(Some(cfg.trading.max_drawdown_pct.to_string())),
        "trading.max_concurrent_positions" => {
            Ok(Some(cfg.trading.max_concurrent_positions.to_string()))
        }
        // research
        "research.iterations" => Ok(Some(cfg.research.iterations.to_string())),
        "research.timeframes" => Ok(Some(cfg.research.timeframes.join(","))),
        "research.max_compile_retries" => Ok(Some(cfg.research.max_compile_retries.to_string())),
        // feedback
        "feedback.min_sharpe_for_promotion" => {
            Ok(Some(cfg.feedback.min_sharpe_for_promotion.to_string()))
        }
        "feedback.min_win_rate" => Ok(Some(cfg.feedback.min_win_rate.to_string())),
        "feedback.min_trades" => Ok(Some(cfg.feedback.min_trades.to_string())),
        "feedback.max_drawdown_for_retirement" => {
            Ok(Some(cfg.feedback.max_drawdown_for_retirement.to_string()))
        }
        // sentinel
        "sentinel.enabled" => Ok(Some(cfg.sentinel.enabled.to_string())),
        "sentinel.check_interval_secs" => {
            Ok(Some(cfg.sentinel.check_interval_secs.to_string()))
        }
        // server
        "server.listen_addr" => Ok(Some(cfg.server.listen_addr.clone())),
        "server.port" => Ok(Some(cfg.server.port.to_string())),
        _ => ConfigSnafu { message: format!("unknown config key: {key}") }.fail(),
    }
}

/// Flatten config into key-value pairs for listing.
fn config_as_map(cfg: &app_config::AppConfig) -> Vec<(String, String)> {
    vec![
        // agent
        ("agent.backend".into(), cfg.agent.backend.clone()),
        (
            "agent.command".into(),
            cfg.agent
                .command
                .as_deref()
                .unwrap_or("(not set)")
                .to_string(),
        ),
        (
            "agent.idle_timeout_secs".into(),
            cfg.agent.idle_timeout_secs.to_string(),
        ),
        // database
        ("database.url".into(), cfg.database.url.clone()),
        // trading
        ("trading.broker".into(), cfg.trading.broker.clone()),
        (
            "trading.contracts".into(),
            cfg.trading.contracts.join(","),
        ),
        (
            "trading.max_position_size".into(),
            cfg.trading.max_position_size.to_string(),
        ),
        (
            "trading.max_drawdown_pct".into(),
            cfg.trading.max_drawdown_pct.to_string(),
        ),
        (
            "trading.max_concurrent_positions".into(),
            cfg.trading.max_concurrent_positions.to_string(),
        ),
        // research
        (
            "research.iterations".into(),
            cfg.research.iterations.to_string(),
        ),
        (
            "research.timeframes".into(),
            cfg.research.timeframes.join(","),
        ),
        (
            "research.max_compile_retries".into(),
            cfg.research.max_compile_retries.to_string(),
        ),
        // feedback
        (
            "feedback.min_sharpe_for_promotion".into(),
            cfg.feedback.min_sharpe_for_promotion.to_string(),
        ),
        (
            "feedback.min_win_rate".into(),
            cfg.feedback.min_win_rate.to_string(),
        ),
        (
            "feedback.min_trades".into(),
            cfg.feedback.min_trades.to_string(),
        ),
        (
            "feedback.max_drawdown_for_retirement".into(),
            cfg.feedback.max_drawdown_for_retirement.to_string(),
        ),
        // sentinel
        (
            "sentinel.enabled".into(),
            cfg.sentinel.enabled.to_string(),
        ),
        (
            "sentinel.check_interval_secs".into(),
            cfg.sentinel.check_interval_secs.to_string(),
        ),
        // server
        ("server.listen_addr".into(), cfg.server.listen_addr.clone()),
        ("server.port".into(), cfg.server.port.to_string()),
    ]
}

/// Execute the feedback subcommand.
fn run_feedback(action: FeedbackAction) -> error::Result<()> {
    match action {
        FeedbackAction::Report { strategy, limit } => {
            run_feedback_report(strategy.as_deref(), limit)
        }

    }
}

/// Display strategy evaluation history from the event bus.
///
/// Reads all feedback-topic events, parses evaluation payloads, optionally
/// filters by strategy ID, sorts by timestamp descending, and prints a
/// human-readable table to stderr and JSON to stdout.
fn run_feedback_report(strategy: Option<&str>, limit: usize) -> error::Result<()> {
    let events_path = paths::data_dir().join("trace/events");
    let event_bus = EventBus::open(&events_path).context(EventBusSnafu)?;

    // Read a large batch of feedback events from the store
    let events = event_bus
        .store()
        .read_topic("feedback", 0, 10_000)
        .context(EventBusSnafu)?;

    let mut entries: Vec<EvaluationEntry> = events
        .into_iter()
        .filter(|e| {
            strategy.is_none_or(|s| e.strategy_id.as_deref() == Some(s))
        })
        .map(|e| {
            let p = &e.payload;
            EvaluationEntry {
                timestamp: e.timestamp.to_string(),
                strategy_id: e
                    .strategy_id
                    .unwrap_or_else(|| "unknown".to_owned()),
                decision: p["decision"]
                    .as_str()
                    .unwrap_or("unknown")
                    .to_owned(),
                reason: p["reason"]
                    .as_str()
                    .unwrap_or("")
                    .to_owned(),
                sharpe_ratio: p["sharpe_ratio"].as_f64().unwrap_or(0.0),
                win_rate: p["win_rate"].as_f64().unwrap_or(0.0),
                trade_count: p["trade_count"].as_u64().unwrap_or(0),
                pnl: p["pnl"]
                    .as_str()
                    .unwrap_or("0")
                    .to_owned(),
                max_drawdown: p["max_drawdown"]
                    .as_str()
                    .unwrap_or("0")
                    .to_owned(),
            }
        })
        .collect();

    // Most recent first
    entries.reverse();
    entries.truncate(limit);

    // Print human-readable table to stderr
    eprintln!(
        "{:<24} {:<16} {:<9} {:>7} {:>9} {:>7} {:>7} {:>10}",
        "Time", "Strategy", "Decision", "Sharpe", "DD", "Win%", "Trades", "PnL"
    );
    for entry in &entries {
        // Format win rate as percentage
        let win_pct = format!("{:.1}%", entry.win_rate * 100.0);
        // Format drawdown
        let dd = format_drawdown(&entry.max_drawdown);
        // Format PnL with sign
        let pnl = format_pnl(&entry.pnl);
        // Truncate timestamp to minutes
        let ts = entry
            .timestamp
            .get(..16)
            .unwrap_or(&entry.timestamp);
        eprintln!(
            "{:<24} {:<16} {:<9} {:>7.2} {:>9} {:>7} {:>7} {:>10}",
            ts,
            entry.strategy_id,
            entry.decision,
            entry.sharpe_ratio,
            dd,
            win_pct,
            entry.trade_count,
            pnl,
        );
    }

    println!(
        "{}",
        serde_json::to_string(&FeedbackReportResponse {
            ok: true,
            action: "feedback.report",
            evaluations: entries,
        })
        .expect("FeedbackReportResponse must serialize")
    );
    Ok(())
}

/// Format a drawdown decimal string as a negative percentage (e.g. "0.05" -> "-5.0%").
fn format_drawdown(dd: &str) -> String {
    dd.parse::<f64>()
        .map_or_else(|_| dd.to_owned(), |v| format!("-{:.1}%", v * 100.0))
}

/// Format a `PnL` string with a sign prefix (e.g. "234.50" -> "+$234", "-124" -> "-$124").
fn format_pnl(pnl: &str) -> String {
    pnl.parse::<f64>().map_or_else(
        |_| pnl.to_owned(),
        |v| {
            if v >= 0.0 {
                format!("+${v:.0}")
            } else {
                format!("-${:.0}", v.abs())
            }
        },
    )
}

/// Execute the data subcommand.
async fn run_data(action: DataAction) -> error::Result<()> {
    match action {
        DataAction::Fetch {
            source,
            symbol,
            start,
            end,
        } => run_data_fetch(&source, &symbol, &start, &end).await,
        DataAction::Info => run_data_info().await,
    }
}

/// Fetch historical market data from an exchange into `TimescaleDB`.
async fn run_data_fetch(
    source: &str,
    symbol: &str,
    start: &str,
    end: &str,
) -> error::Result<()> {
    let start_date = NaiveDate::parse_from_str(start, "%Y-%m-%d").map_err(|_| {
        error::AppError::Config {
            message: format!("invalid start date: {start}"),
        }
    })?;
    let end_date = NaiveDate::parse_from_str(end, "%Y-%m-%d").map_err(|_| {
        error::AppError::Config {
            message: format!("invalid end date: {end}"),
        }
    })?;

    let cfg = app_config::load();
    let store = rara_market_data::store::MarketStore::connect(&cfg.database.url)
        .await
        .context(MarketStoreSnafu)?;
    store.migrate().await.context(MarketStoreSnafu)?;

    let fetcher: Box<dyn rara_market_data::fetcher::HistoryFetcher> = match source {
        "binance" => Box::new(rara_market_data::fetcher::binance::BinanceFetcher::new(
            symbol,
        )),
        "yahoo" => Box::new(rara_market_data::fetcher::yahoo::YahooFetcher::new(symbol)),
        _ => {
            return ConfigSnafu {
                message: format!("unknown source: {source}, expected 'binance' or 'yahoo'"),
            }
            .fail();
        }
    };

    let instrument_id = symbol;
    let count = fetcher
        .fetch_and_store(&store, instrument_id, start_date, end_date)
        .await
        .context(DataFetchSnafu)?;

    tracing::info!(count, instrument_id, source, "data fetch completed");
    println!(
        "{}",
        serde_json::to_string(&DataFetchResponse {
            ok: true,
            action: "data.fetch",
            source,
            symbol,
            candles: count,
        })
        .expect("DataFetchResponse must serialize")
    );
    Ok(())
}

/// Show data coverage for all stored instruments.
async fn run_data_info() -> error::Result<()> {
    let cfg = app_config::load();
    let store = rara_market_data::store::MarketStore::connect(&cfg.database.url)
        .await
        .context(MarketStoreSnafu)?;
    store.migrate().await.context(MarketStoreSnafu)?;

    let coverage = store.get_coverage().await.context(MarketStoreSnafu)?;

    println!(
        "{}",
        serde_json::to_string(&DataInfoResponse {
            ok: true,
            action: "data.info",
            instruments: coverage,
        })
        .expect("DataInfoResponse must serialize")
    );
    Ok(())
}

/// Execute the research subcommand.
async fn run_research(action: ResearchAction) -> error::Result<()> {
    match action {
        ResearchAction::Run {
            iterations,
            contract,
            trace_dir,
            quiet,
        } => run_research_loop(iterations, &contract, trace_dir, quiet).await,
        ResearchAction::List { limit, trace_dir } => run_research_list(limit, trace_dir),
        ResearchAction::Show {
            experiment_id,
            trace_dir,
        } => run_research_show(&experiment_id, trace_dir),
        ResearchAction::Promoted { promoted_dir } => run_research_promoted(promoted_dir),
    }
}

/// Build the `ResearchLoop` from config, trace path, and DB connection.
async fn build_research_loop(trace_path: &Path) -> error::Result<ResearchLoop> {
    let template_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("strategies/template");
    let prompts_dir =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("crates/rara-research/src/prompts");

    let trace = Trace::open(trace_path).context(TraceSnafu)?;
    let event_bus = Arc::new(EventBus::open(&trace_path.join("events")).context(EventBusSnafu)?);
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

    let backtester: Arc<dyn rara_trading::research::backtester::Backtester> =
        Arc::new(BarterBacktester::builder()
            .store(market_store)
            .initial_capital(dec!(10000))
            .fees_percent(dec!(0.1))
            .backtest_start(NaiveDate::from_ymd_opt(2020, 1, 1).expect("valid date"))
            .backtest_end(NaiveDate::from_ymd_opt(2030, 12, 31).expect("valid date"))
            .build());

    let cli_backend =
        CliBackend::from_agent_config(&cfg.agent).context(error::AgentBackendSnafu)?;
    let llm: Arc<dyn rara_trading::infra::llm::LlmClient> =
        Arc::new(CliExecutor::new(cli_backend));

    let strategy_db_path = trace_path.join("strategy_db");
    let artifact_dir = paths::data_dir().join("artifacts");
    let strategy_store = StrategyStore::open_path(&strategy_db_path, &artifact_dir)
        .expect("failed to open strategy store");

    let strategy_manager: Arc<dyn rara_trading::research::strategy_manager::StrategyManager> =
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

/// Run N iterations of the research loop.
async fn run_research_loop(
    iterations: u32,
    contract: &str,
    trace_dir: Option<String>,
    quiet: bool,
) -> error::Result<()> {
    let trace_path = trace_dir.map_or_else(|| paths::data_dir().join("trace"), PathBuf::from);
    let research_loop = build_research_loop(&trace_path).await?;

    let mut accepted_count: u32 = 0;
    let mut rejected_count: u32 = 0;
    let mut error_count: u32 = 0;

    for i in 1..=iterations {
        tracing::info!(iteration = i, total = iterations, "research iteration starting");
        let result = research_loop.run_iteration(contract).await;
        match result {
            Ok(ir) => {
                if ir.accepted {
                    accepted_count += 1;
                } else {
                    rejected_count += 1;
                }

                tracing::info!(
                    iteration = i,
                    total = iterations,
                    accepted = ir.accepted,
                    hypothesis = %ir.hypothesis.text,
                    "research iteration completed"
                );

                if !quiet {
                    let hyp_summary: String = ir.hypothesis.text.chars().take(60).collect();
                    eprintln!("[{i}/{iterations}] Hypothesis: {hyp_summary}...");

                    if let Some(ref bt) = ir.experiment.backtest_result {
                        eprintln!(
                            "       Backtest: sharpe={:.2} win={:.0}% trades={} pnl={}",
                            bt.sharpe_ratio,
                            bt.win_rate * 100.0,
                            bt.trade_count,
                            bt.pnl,
                        );
                    }

                    let status = if ir.accepted { "ACCEPTED" } else { "rejected" };
                    eprintln!("       Result: {status}");
                }

                println!(
                    "{}",
                    serde_json::to_string(&IterationResponse {
                        iteration: i,
                        accepted: ir.accepted,
                        hypothesis: &ir.hypothesis.text,
                    })
                    .expect("IterationResponse must serialize")
                );
            }
            Err(e) => {
                error_count += 1;
                tracing::error!(
                    iteration = i,
                    total = iterations,
                    error = %e,
                    "research iteration failed"
                );
                if !quiet {
                    eprintln!("[{i}/{iterations}] ERROR: {e}");
                }
            }
        }
    }

    eprintln!("=== Research Summary ===");
    eprintln!("Total: {iterations} | Accepted: {accepted_count} | Rejected: {rejected_count} | Errors: {error_count}");

    println!(
        "{}",
        serde_json::to_string(&ResearchRunResponse {
            ok: true,
            action: "research.run",
            iterations,
            accepted: accepted_count,
            rejected: rejected_count,
            errors: error_count,
        })
        .expect("ResearchRunResponse must serialize")
    );
    Ok(())
}

/// List recent experiments from the trace store.
fn run_research_list(limit: usize, trace_dir: Option<String>) -> error::Result<()> {
    let trace_path = trace_dir.map_or_else(|| paths::data_dir().join("trace"), PathBuf::from);
    let trace = Trace::open(&trace_path).context(TraceSnafu)?;

    let entries = trace.list_recent(limit).context(TraceSnafu)?;

    let items: Vec<ExperimentListItem> = entries
        .into_iter()
        .map(|(idx, exp, fb)| {
            let hypothesis_text = trace
                .get_hypothesis(exp.hypothesis_id)
                .ok()
                .flatten()
                .map_or_else(|| "unknown".to_owned(), |h| h.text);

            let decision = fb.as_ref().map_or("no feedback", |f| {
                if f.decision {
                    "accepted"
                } else {
                    "rejected"
                }
            });

            let sharpe = exp
                .backtest_result
                .as_ref()
                .map(|result| result.sharpe_ratio);

            ExperimentListItem {
                index: idx,
                experiment_id: exp.id.to_string(),
                hypothesis: hypothesis_text,
                decision,
                sharpe,
            }
        })
        .collect();

    println!(
        "{}",
        serde_json::to_string(&ResearchListResponse {
            ok: true,
            action: "research.list",
            experiments: items,
        })
        .expect("ResearchListResponse must serialize")
    );
    Ok(())
}

/// Show full details of a specific experiment.
fn run_research_show(experiment_id: &str, trace_dir: Option<String>) -> error::Result<()> {
    let trace_path = trace_dir.map_or_else(|| paths::data_dir().join("trace"), PathBuf::from);
    let trace = Trace::open(&trace_path).context(TraceSnafu)?;

    let exp_uuid = Uuid::parse_str(experiment_id).map_err(|_| error::AppError::Config {
        message: format!("invalid experiment ID: {experiment_id}"),
    })?;

    let exp = trace
        .get_experiment(exp_uuid)
        .context(TraceSnafu)?
        .ok_or_else(|| error::AppError::Config {
            message: format!("experiment not found: {experiment_id}"),
        })?;

    let hypothesis = trace
        .get_hypothesis(exp.hypothesis_id)
        .context(TraceSnafu)?;

    let feedbacks = trace
        .get_feedback_for_experiment(exp_uuid)
        .context(TraceSnafu)?;

    let hyp_detail = hypothesis.map(|h| HypothesisDetail {
        id: h.id.to_string(),
        text: h.text,
        reason: h.reason,
        observation: h.observation,
        knowledge: h.knowledge,
        parent: h.parent.map(|p| p.to_string()),
    });

    let fb_details: Vec<FeedbackDetail> = feedbacks
        .iter()
        .map(|fb| FeedbackDetail {
            experiment_id: fb.experiment_id.to_string(),
            decision: fb.decision,
            reason: fb.reason.clone(),
            observations: fb.observations.clone(),
            hypothesis_evaluation: fb.hypothesis_evaluation.clone(),
            new_hypothesis: fb.new_hypothesis.clone(),
            code_change_summary: fb.code_change_summary.clone(),
        })
        .collect();

    let backtest_detail = exp.backtest_result.as_ref().map(|br| BacktestDetail {
        pnl: br.pnl.to_string(),
        sharpe_ratio: br.sharpe_ratio,
        max_drawdown: br.max_drawdown.to_string(),
        win_rate: br.win_rate,
        trade_count: br.trade_count,
    });

    println!(
        "{}",
        serde_json::to_string(&ResearchShowResponse {
            ok: true,
            action: "research.show",
            experiment: ExperimentDetail {
                id: exp.id.to_string(),
                hypothesis_id: exp.hypothesis_id.to_string(),
                status: exp.status.to_string(),
                strategy_code: exp.strategy_code,
                backtest_result: backtest_detail,
            },
            hypothesis: hyp_detail,
            feedbacks: fb_details,
        })
        .expect("ResearchShowResponse must serialize")
    );
    Ok(())
}

/// List promoted strategies from the promoted directory.
fn run_research_promoted(promoted_dir: Option<String>) -> error::Result<()> {
    let dir = promoted_dir.map_or_else(paths::strategies_promoted_dir, PathBuf::from);

    let promoted = list_promoted_from_dir(&dir).context(PromoterSnafu)?;

    let items: Vec<PromotedItem> = promoted
        .iter()
        .map(|p| PromotedItem {
            experiment_id: p.experiment_id().to_string(),
            hypothesis_id: p.hypothesis_id().to_string(),
            wasm_path: p.wasm_path().to_string_lossy().into_owned(),
            source_path: p.source_path().map(|s| s.to_string_lossy().into_owned()),
            meta: PromotedMeta {
                name: p.meta().name.clone(),
                version: p.meta().version,
                api_version: p.meta().api_version,
                description: p.meta().description.clone(),
            },
        })
        .collect();

    println!(
        "{}",
        serde_json::to_string(&ResearchPromotedResponse {
            ok: true,
            action: "research.promoted",
            strategies: items,
        })
        .expect("ResearchPromotedResponse must serialize")
    );
    Ok(())
}

/// Start the gRPC server on the given port.
async fn run_serve(port: u16) -> error::Result<()> {
    use rara_server::rara_proto::rara_service_server::RaraServiceServer;
    use rara_server::service::RaraServiceImpl;

    let addr = format!("0.0.0.0:{port}")
        .parse::<std::net::SocketAddr>()
        .map_err(|_| error::AppError::Config {
            message: format!("invalid port: {port}"),
        })?;

    eprintln!("gRPC server listening on {addr}");

    tonic::transport::Server::builder()
        .add_service(RaraServiceServer::new(RaraServiceImpl::new()))
        .serve(addr)
        .await
        .context(GrpcServeSnafu)?;

    Ok(())
}


/// Execute the paper trading subcommand.
async fn run_paper(action: PaperAction) -> error::Result<()> {
    match action {
        PaperAction::Start { contracts } => run_paper_start(contracts).await,
        PaperAction::Status => run_paper_status(),
    }
}

/// Show paper trading status by reading trading events from the event bus.
///
/// Aggregates order-submitted, order-filled, and order-rejected events by
/// strategy and prints a summary table to stderr plus JSON to stdout.
fn run_paper_status() -> error::Result<()> {
    use rara_domain::event::EventType;
    use std::collections::BTreeMap;

    let trace_path = paths::data_dir().join("trace");
    let event_bus_path = trace_path.join("events");

    if !event_bus_path.exists() {
        eprintln!("No event bus data found. Has paper trading been run?");
        println!(
            "{}",
            serde_json::to_string(&PaperStatusResponse {
                ok: true,
                action: "paper.status",
                strategies: vec![],
                total_trades: 0,
            })
            .expect("PaperStatusResponse must serialize")
        );
        return Ok(());
    }

    let event_bus = EventBus::open(&event_bus_path).context(EventBusSnafu)?;
    let events = event_bus.store().read_topic("trading", 0, 100_000).context(EventBusSnafu)?;

    // Aggregate by strategy_id
    let mut stats: BTreeMap<String, (usize, usize, usize)> = BTreeMap::new();
    for event in &events {
        let strategy = event
            .strategy_id
            .as_deref()
            .unwrap_or("unknown")
            .to_string();
        let entry = stats.entry(strategy).or_insert((0, 0, 0));
        match event.event_type {
            EventType::TradingOrderSubmitted => entry.0 += 1,
            EventType::TradingOrderFilled => entry.1 += 1,
            EventType::TradingOrderRejected => entry.2 += 1,
            _ => {}
        }
    }

    let strategies: Vec<StrategyStatus> = stats
        .into_iter()
        .map(|(name, (trades, filled, rejected))| StrategyStatus {
            strategy: name,
            trades,
            filled,
            rejected,
        })
        .collect();

    let total_trades: usize = strategies.iter().map(|s| s.trades).sum();

    // Human-readable table to stderr
    eprintln!(
        "{:<20} {:>8} {:>8} {:>8}",
        "Strategy", "Trades", "Filled", "Rejected"
    );
    eprintln!("{}", "-".repeat(48));
    for s in &strategies {
        eprintln!(
            "{:<20} {:>8} {:>8} {:>8}",
            s.strategy, s.trades, s.filled, s.rejected
        );
    }
    if strategies.is_empty() {
        eprintln!("(no trading events recorded)");
    }

    println!(
        "{}",
        serde_json::to_string(&PaperStatusResponse {
            ok: true,
            action: "paper.status",
            strategies,
            total_trades,
        })
        .expect("PaperStatusResponse must serialize")
    );
    Ok(())
}

/// Load promoted WASM strategies for the given contracts.
///
/// Reads all promoted strategy definitions from `promoted_dir`, compiles each
/// into a WASM handle per contract, and returns the resulting
/// [`LoadedStrategy`] instances.
fn load_strategies_for_contracts(
    contracts: &[String],
    position_size: rust_decimal::Decimal,
) -> error::Result<Vec<rara_trading::trading::signal_loop::LoadedStrategy>> {
    use rara_trading::trading::signal_loop::LoadedStrategy;

    let promoted_dir = paths::strategies_promoted_dir();
    let promoted = list_promoted_from_dir(&promoted_dir).context(PromoterSnafu)?;

    if promoted.is_empty() {
        return Ok(vec![]);
    }

    let executor = WasmExecutor::builder().build();
    let mut loaded = Vec::new();

    for p in &promoted {
        let wasm_bytes = match std::fs::read(p.wasm_path()) {
            Ok(b) => b,
            Err(e) => {
                tracing::error!(strategy = p.meta().name, error = %e, "failed to read WASM file");
                continue;
            }
        };

        for contract in contracts {
            match executor.load(&wasm_bytes) {
                Ok(handle) => {
                    loaded.push(LoadedStrategy {
                        name: p.meta().name.clone(),
                        version: p.meta().version,
                        contract_id: contract.clone(),
                        position_size,
                        handle,
                    });
                }
                Err(e) => {
                    tracing::error!(
                        strategy = p.meta().name, contract, error = ?e,
                        "failed to load WASM handle"
                    );
                }
            }
        }
    }

    Ok(loaded)
}

/// Start the paper trading main loop.
///
/// Connects to Binance WebSocket for live kline data, loads promoted WASM
/// strategies, and runs the signal loop through a paper broker. Blocks until
/// Ctrl+C is received, then gracefully shuts down all tasks.
async fn run_paper_start(contracts_override: Option<String>) -> error::Result<()> {
    use futures_util::StreamExt;
    use rara_trading::trading::brokers::paper::PaperBroker;
    use rara_trading::trading::engine::TradingEngine;
    use rara_trading::trading::guard_pipeline::GuardPipeline;
    use rara_trading::trading::signal_loop::run_signal_loop;
    use rara_market_data::stream::aggregator::CandleAggregator;
    use rara_market_data::stream::binance_ws::BinanceWsClient;

    let cfg = app_config::load();
    let contracts: Vec<String> = contracts_override.map_or_else(
        || cfg.trading.contracts.clone(),
        |c| c.split(',').map(|s| s.trim().to_string()).collect(),
    );

    let position_size =
        rust_decimal::Decimal::try_from(cfg.trading.max_position_size).unwrap_or(dec!(1));
    let loaded_strategies = load_strategies_for_contracts(&contracts, position_size)?;

    if loaded_strategies.is_empty() {
        eprintln!("No promoted strategies found. Run 'rara research run' first.");
        return Ok(());
    }

    eprintln!(
        "Loaded {} strategy instances for {} contracts",
        loaded_strategies.len(),
        contracts.len()
    );

    // Open event bus + build trading engine
    let trace_path = paths::data_dir().join("trace");
    let event_bus = Arc::new(EventBus::open(&trace_path.join("events")).context(EventBusSnafu)?);
    let broker: Box<dyn rara_trading::trading::broker::Broker> =
        Box::new(PaperBroker::new(dec!(0)));
    let guard_pipeline = GuardPipeline::new(vec![]);
    let engine = Arc::new(TradingEngine::new(broker, guard_pipeline, Arc::clone(&event_bus)));

    // Setup candle aggregator + WebSocket connection
    let (mut aggregator, candle_rx) = CandleAggregator::with_defaults();
    let ws_client = BinanceWsClient::new();
    let subs: Vec<(&str, &str)> = contracts.iter().map(|c| (c.as_str(), "1m")).collect();
    let mut kline_stream = ws_client
        .subscribe_klines_multi(&subs)
        .await
        .map_err(|e| error::AppError::Config {
            message: format!("WebSocket connection failed: {e}"),
        })?;

    // Spawn aggregator: forward raw klines into candle aggregator
    let agg_handle = tokio::spawn(async move {
        while let Some(item) = kline_stream.next().await {
            match item {
                Ok(kline) => aggregator.process_kline(&kline),
                Err(e) => {
                    tracing::error!(error = %e, "kline stream error");
                    break;
                }
            }
        }
        tracing::info!("kline stream ended");
    });

    // Spawn signal loop
    let signal_handle = tokio::spawn(async move {
        run_signal_loop(candle_rx, engine, loaded_strategies).await;
    });

    eprintln!("Paper trading started. Press Ctrl+C to stop.");
    tokio::signal::ctrl_c()
        .await
        .expect("failed to listen for ctrl+c");
    eprintln!("\nShutting down...");

    agg_handle.abort();
    signal_handle.abort();

    eprintln!("Paper trading stopped.");
    Ok(())
}

/// Read promoted strategy metadata files from a directory without requiring
/// the full `StrategyPromoter` (which needs trace, runtime, and compiler).
fn list_promoted_from_dir(
    dir: &Path,
) -> rara_trading::research::strategy_promoter::Result<Vec<PromotedStrategy>> {
    use rara_trading::research::strategy_promoter::{IoSnafu as PmIoSnafu, SerializeSnafu as PmSerializeSnafu};

    if !dir.exists() {
        return Ok(vec![]);
    }

    let mut promoted = Vec::new();
    let entries = std::fs::read_dir(dir).context(PmIoSnafu)?;

    for entry in entries {
        let entry = entry.context(PmIoSnafu)?;
        let path = entry.path();

        if path.extension().is_some_and(|ext| ext == "json") {
            let contents = std::fs::read_to_string(&path).context(PmIoSnafu)?;
            let strategy: PromotedStrategy =
                serde_json::from_str(&contents).context(PmSerializeSnafu)?;
            promoted.push(strategy);
        }
    }

    Ok(promoted)
}
