#![allow(clippy::result_large_err)]

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use chrono::NaiveDate;
use clap::Parser;
use rara_trading::{
    accounts_config,
    agent::{CliBackend, CliExecutor},
    app_config,
    cli::{
        Cli, Command, ConfigAction, DataAction, EventsAction, FeedbackAction, PaperAction,
        ResearchAction, SetupAccountAction, SetupAction, StrategyAction,
    },
    error::{
        self, AgentBackendSnafu, AgentExecutionSnafu, AppError, ConfigSnafu, DataFetchSnafu,
        EventBusSnafu, GrpcServeSnafu, IoSnafu, MarketStoreSnafu, PromoterSnafu,
        PromptRendererSnafu, RegistrySnafu, TraceSnafu,
    },
    event_bus::bus::EventBus,
    logging::{self, LoggingConfig},
    paths, validation,
};
use rust_decimal_macros::dec;
use serde::Serialize;
use snafu::ResultExt;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// CLI response types — compile-time typed JSON output
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct ErrorResponse {
    ok:         bool,
    error:      String,
    #[serde(skip_serializing_if = "Option::is_none")]
    suggestion: Option<String>,
}

#[derive(Serialize)]
struct ConfigSetResponse<'a> {
    ok:     bool,
    action: &'static str,
    key:    &'a str,
    value:  &'a str,
}

#[derive(Serialize)]
struct ConfigGetResponse<'a> {
    ok:     bool,
    action: &'static str,
    key:    &'a str,
    value:  &'a str,
}

#[derive(Serialize)]
struct ConfigListResponse {
    ok:      bool,
    action:  &'static str,
    entries: serde_json::Map<String, serde_json::Value>,
}

#[derive(Serialize)]
struct SetupInitResponse {
    ok:      bool,
    action:  &'static str,
    created: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason:  Option<String>,
}

#[derive(Serialize)]
struct ValidateCheck {
    name:       String,
    ok:         bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    detail:     Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    suggestion: Option<String>,
}

#[derive(Serialize)]
struct SetupValidateResponse {
    ok:     bool,
    action: &'static str,
    checks: Vec<ValidateCheck>,
}

#[derive(Serialize)]
struct SetupAccountAddResponse<'a> {
    ok:      bool,
    action:  &'static str,
    id:      &'a str,
    created: bool,
}

#[derive(Serialize)]
struct SetupAccountListResponse {
    ok:       bool,
    action:   &'static str,
    accounts: Vec<serde_json::Value>,
}

#[derive(Serialize)]
struct SetupAccountRemoveResponse<'a> {
    ok:      bool,
    action:  &'static str,
    id:      &'a str,
    removed: bool,
}

#[derive(Serialize)]
struct SetupAccountTestResponse<'a> {
    ok:             bool,
    action:         &'static str,
    id:             &'a str,
    equity:         String,
    available_cash: String,
}

#[derive(Serialize)]
struct HelloResponse<'a> {
    ok:       bool,
    action:   &'static str,
    greeting: &'a str,
}

#[derive(Serialize)]
struct AgentResponse<'a> {
    ok:        bool,
    action:    &'static str,
    exit_code: Option<i32>,
    timed_out: bool,
    output:    &'a str,
}

#[derive(Serialize)]
struct IterationResponse<'a> {
    iteration:  u32,
    accepted:   bool,
    hypothesis: &'a str,
}

#[derive(Serialize)]
struct ResearchRunResponse {
    ok:         bool,
    action:     &'static str,
    iterations: u32,
    accepted:   u32,
    rejected:   u32,
    errors:     u32,
}

#[derive(Serialize)]
struct DataFetchResponse<'a> {
    ok:      bool,
    action:  &'static str,
    source:  &'a str,
    symbol:  &'a str,
    candles: usize,
}

#[derive(Serialize)]
struct DataInfoResponse {
    ok:          bool,
    action:      &'static str,
    instruments: Vec<rara_market_data::store::candle::CandleCoverage>,
}

#[derive(Serialize)]
struct ExperimentListItem {
    index:         u64,
    experiment_id: String,
    hypothesis:    String,
    decision:      &'static str,
    sharpe:        Option<f64>,
}

#[derive(Serialize)]
struct ResearchListResponse {
    ok:          bool,
    action:      &'static str,
    experiments: Vec<ExperimentListItem>,
}

#[derive(Serialize)]
struct HypothesisDetail {
    id:          String,
    text:        String,
    reason:      String,
    observation: String,
    knowledge:   String,
    parent:      Option<String>,
}

#[derive(Serialize)]
struct FeedbackDetail {
    experiment_id:         String,
    decision:              bool,
    reason:                String,
    observations:          String,
    hypothesis_evaluation: String,
    new_hypothesis:        Option<String>,
    code_change_summary:   String,
}

#[derive(Serialize)]
struct BacktestDetail {
    pnl:          String,
    sharpe_ratio: f64,
    max_drawdown: String,
    win_rate:     f64,
    trade_count:  u32,
}

#[derive(Serialize)]
struct ExperimentDetail {
    id:              String,
    hypothesis_id:   String,
    status:          String,
    strategy_code:   String,
    backtest_result: Option<BacktestDetail>,
}

#[derive(Serialize)]
struct ResearchShowResponse {
    ok:         bool,
    action:     &'static str,
    experiment: ExperimentDetail,
    hypothesis: Option<HypothesisDetail>,
    feedbacks:  Vec<FeedbackDetail>,
}

#[derive(Serialize)]
struct PromotedItem {
    experiment_id: String,
    hypothesis_id: String,
    wasm_path:     String,
    source_path:   Option<String>,
    meta:          PromotedMeta,
}

#[derive(Serialize)]
struct PromotedMeta {
    name:        String,
    version:     u32,
    api_version: u32,
    description: String,
}

#[derive(Serialize)]
struct ResearchPromotedResponse {
    ok:         bool,
    action:     &'static str,
    strategies: Vec<PromotedItem>,
}

#[derive(Serialize)]
struct EvaluationEntry {
    timestamp:    String,
    strategy_id:  String,
    decision:     String,
    reason:       String,
    sharpe_ratio: f64,
    win_rate:     f64,
    trade_count:  u64,
    pnl:          String,
    max_drawdown: String,
}

#[derive(Serialize)]
struct FeedbackReportResponse {
    ok:          bool,
    action:      &'static str,
    evaluations: Vec<EvaluationEntry>,
}

/// Per-strategy aggregated status from event bus trading events.
#[derive(Serialize)]
struct StrategyStatus {
    strategy: String,
    trades:   usize,
    filled:   usize,
    rejected: usize,
}

/// Response payload for `paper status`.
#[derive(Serialize)]
struct PaperStatusResponse {
    ok:           bool,
    action:       &'static str,
    strategies:   Vec<StrategyStatus>,
    total_trades: usize,
}

/// Response payload for `paper stop`.
#[derive(Serialize)]
struct PaperStopResponse {
    ok:      bool,
    action:  &'static str,
    message: String,
}

/// Summary printed after graceful shutdown of paper trading.
#[derive(Serialize)]
struct PaperShutdownSummary {
    ok:            bool,
    action:        &'static str,
    duration_secs: u64,
    total_trades:  usize,
}

use rara_trading::research::{
    barter_backtester::BarterBacktester, compiler::StrategyCompiler,
    feedback_gen::FeedbackGenerator, hypothesis_gen::HypothesisGenerator,
    prompt_renderer::PromptRenderer, research_loop::ResearchLoop, strategy_coder::StrategyCoder,
    strategy_executor::StrategyExecutor, strategy_promoter::PromotedStrategy,
    strategy_store::StrategyStore, trace::Trace, wasm_executor::WasmExecutor,
    wasm_strategy_manager::WasmStrategyManager,
};

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
                ok:         false,
                error:      e.to_string(),
                suggestion: None,
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
            ConfigAction::Set { key, value } => {
                let mut cfg = app_config::load().clone();
                set_config_field(&mut cfg, &key, &value)?;
                app_config::save(&cfg).context(IoSnafu)?;
                tracing::info!(key = %key, value = %value, "config updated");
                println!(
                    "{}",
                    serde_json::to_string(&ConfigSetResponse {
                        ok:     true,
                        action: "config_set",
                        key:    &key,
                        value:  &value,
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
                        ok:     true,
                        action: "config_get",
                        key:    &key,
                        value:  display_value,
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
                        ok:      true,
                        action:  "config_list",
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
                    ok:       true,
                    action:   "hello",
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
            iterations,
            grpc_addr,
        } => {
            rara_trading::daemon::run(iterations, grpc_addr).await?;
        }
        Command::Setup {
            interactive,
            action,
        } => {
            if interactive {
                rara_trading::setup_wizard::run().await?;
            } else if let Some(action) = action {
                run_setup(action).await?;
            } else {
                eprintln!("usage: rara setup <init|account|validate> or rara setup -i");
                std::process::exit(1);
            }
        }
        Command::Serve { port } => {
            run_serve(port).await?;
        }
        Command::Tui { server } => {
            rara_tui::event_loop::run(server.as_deref(), crate::paths::strategies_promoted_dir())
                .await
                .map_err(|e| AppError::Tui {
                    source: Box::new(e),
                })?;
        }
        Command::Feedback { action } => {
            run_feedback(action)?;
        }
        Command::Paper { action } => {
            run_paper(action).await?;
        }
        Command::Strategy { action } => {
            run_strategy(action).await?;
        }
        Command::Events { action } => {
            run_events(action)?;
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
                    ok:        result.success,
                    action:    "agent_run",
                    exit_code: result.exit_code,
                    timed_out: result.timed_out,
                    output:    &result.output,
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
        _ => {
            return ConfigSnafu {
                message: format!("unknown config key: {key}"),
            }
            .fail();
        }
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
        "sentinel.check_interval_secs" => Ok(Some(cfg.sentinel.check_interval_secs.to_string())),
        // server
        "server.listen_addr" => Ok(Some(cfg.server.listen_addr.clone())),
        "server.port" => Ok(Some(cfg.server.port.to_string())),
        _ => ConfigSnafu {
            message: format!("unknown config key: {key}"),
        }
        .fail(),
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
        ("sentinel.enabled".into(), cfg.sentinel.enabled.to_string()),
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
        .filter(|e| strategy.is_none_or(|s| e.strategy_id.as_deref() == Some(s)))
        .map(|e| {
            let p = &e.payload;
            EvaluationEntry {
                timestamp:    e.timestamp.to_string(),
                strategy_id:  e.strategy_id.unwrap_or_else(|| "unknown".to_owned()),
                decision:     p["decision"].as_str().unwrap_or("unknown").to_owned(),
                reason:       p["reason"].as_str().unwrap_or("").to_owned(),
                sharpe_ratio: p["sharpe_ratio"].as_f64().unwrap_or(0.0),
                win_rate:     p["win_rate"].as_f64().unwrap_or(0.0),
                trade_count:  p["trade_count"].as_u64().unwrap_or(0),
                pnl:          p["pnl"].as_str().unwrap_or("0").to_owned(),
                max_drawdown: p["max_drawdown"].as_str().unwrap_or("0").to_owned(),
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
        let ts = entry.timestamp.get(..16).unwrap_or(&entry.timestamp);
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
            ok:          true,
            action:      "feedback.report",
            evaluations: entries,
        })
        .expect("FeedbackReportResponse must serialize")
    );
    Ok(())
}

/// Format a drawdown decimal string as a negative percentage (e.g. "0.05" ->
/// "-5.0%").
fn format_drawdown(dd: &str) -> String {
    dd.parse::<f64>()
        .map_or_else(|_| dd.to_owned(), |v| format!("-{:.1}%", v * 100.0))
}

/// Format a `PnL` string with a sign prefix (e.g. "234.50" -> "+$234", "-124"
/// -> "-$124").
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
async fn run_data_fetch(source: &str, symbol: &str, start: &str, end: &str) -> error::Result<()> {
    let start_date =
        NaiveDate::parse_from_str(start, "%Y-%m-%d").map_err(|_| error::AppError::Config {
            message: format!("invalid start date: {start}"),
        })?;
    let end_date =
        NaiveDate::parse_from_str(end, "%Y-%m-%d").map_err(|_| error::AppError::Config {
            message: format!("invalid end date: {end}"),
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
            ok:          true,
            action:      "data.info",
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
async fn build_research_loop(trace_path: &Path, contract: &str) -> error::Result<ResearchLoop> {
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

    let backtester: Arc<dyn rara_trading::research::backtester::Backtester> = Arc::new(
        BarterBacktester::builder()
            .store(market_store)
            .initial_capital(dec!(10000))
            .fees_percent(dec!(0.1))
            .backtest_start(NaiveDate::from_ymd_opt(2020, 1, 1).expect("valid date"))
            .backtest_end(NaiveDate::from_ymd_opt(2030, 12, 31).expect("valid date"))
            .build(),
    );

    let cli_backend =
        CliBackend::from_agent_config(&cfg.agent).context(error::AgentBackendSnafu)?;
    let llm: Arc<dyn rara_trading::infra::llm::LlmClient> = Arc::new(CliExecutor::new(cli_backend));

    let strategy_db_path = trace_path.join("strategy_db");
    let artifact_dir = paths::data_dir().join("artifacts");
    let strategy_store = StrategyStore::open_path(&strategy_db_path, &artifact_dir)
        .expect("failed to open strategy store");

    let strategy_manager: Arc<dyn rara_trading::research::strategy_manager::StrategyManager> =
        Arc::new(
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

/// Run N iterations of the research loop.
async fn run_research_loop(
    iterations: u32,
    contract: &str,
    trace_dir: Option<String>,
    quiet: bool,
) -> error::Result<()> {
    let trace_path = trace_dir.map_or_else(|| paths::data_dir().join("trace"), PathBuf::from);
    let research_loop = build_research_loop(&trace_path, contract).await?;

    let mut accepted_count: u32 = 0;
    let mut rejected_count: u32 = 0;
    let mut error_count: u32 = 0;

    for i in 1..=iterations {
        tracing::info!(
            iteration = i,
            total = iterations,
            "research iteration starting"
        );
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
                        iteration:  i,
                        accepted:   ir.accepted,
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
    eprintln!(
        "Total: {iterations} | Accepted: {accepted_count} | Rejected: {rejected_count} | Errors: \
         {error_count}"
    );

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
                if f.decision { "accepted" } else { "rejected" }
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
            ok:          true,
            action:      "research.list",
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
        id:          h.id.to_string(),
        text:        h.text,
        reason:      h.reason,
        observation: h.observation,
        knowledge:   h.knowledge,
        parent:      h.parent.map(|p| p.to_string()),
    });

    let fb_details: Vec<FeedbackDetail> = feedbacks
        .iter()
        .map(|fb| FeedbackDetail {
            experiment_id:         fb.experiment_id.to_string(),
            decision:              fb.decision,
            reason:                fb.reason.clone(),
            observations:          fb.observations.clone(),
            hypothesis_evaluation: fb.hypothesis_evaluation.clone(),
            new_hypothesis:        fb.new_hypothesis.clone(),
            code_change_summary:   fb.code_change_summary.clone(),
        })
        .collect();

    let backtest_detail = exp.backtest_result.as_ref().map(|br| BacktestDetail {
        pnl:          br.pnl.to_string(),
        sharpe_ratio: br.sharpe_ratio,
        max_drawdown: br.max_drawdown.to_string(),
        win_rate:     br.win_rate,
        trade_count:  br.trade_count,
    });

    println!(
        "{}",
        serde_json::to_string(&ResearchShowResponse {
            ok:         true,
            action:     "research.show",
            experiment: ExperimentDetail {
                id:              exp.id.to_string(),
                hypothesis_id:   exp.hypothesis_id.to_string(),
                status:          exp.status.to_string(),
                strategy_code:   exp.strategy_code,
                backtest_result: backtest_detail,
            },
            hypothesis: hyp_detail,
            feedbacks:  fb_details,
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
            wasm_path:     p.wasm_path().to_string_lossy().into_owned(),
            source_path:   p.source_path().map(|s| s.to_string_lossy().into_owned()),
            meta:          PromotedMeta {
                name:        p.meta().name.clone(),
                version:     p.meta().version,
                api_version: p.meta().api_version,
                description: p.meta().description.clone(),
            },
        })
        .collect();

    println!(
        "{}",
        serde_json::to_string(&ResearchPromotedResponse {
            ok:         true,
            action:     "research.promoted",
            strategies: items,
        })
        .expect("ResearchPromotedResponse must serialize")
    );
    Ok(())
}

/// Start the gRPC server on the given port.
///
/// In standalone mode (launched by `rara tui`), this is the "full stack" entry
/// point: gRPC server + market data WebSocket connection so that all TUI
/// health indicators reflect real service state.
async fn run_serve(port: u16) -> error::Result<()> {
    use std::sync::{Arc, atomic::AtomicBool};

    use rara_server::{
        health::HealthConfig, rara_proto::rara_service_server::RaraServiceServer,
        service::RaraServiceImpl,
    };

    let cfg = crate::app_config::load();

    let ws_connected = Arc::new(AtomicBool::new(false));

    // Collect contracts from enabled accounts for market data subscriptions
    let accounts_cfg = crate::accounts_config::load_accounts();
    let contracts: Vec<String> = accounts_cfg
        .accounts
        .iter()
        .filter(|a| a.enabled)
        .flat_map(|a| a.contracts.clone())
        .collect();

    let health_config = HealthConfig {
        database_url:   cfg.database.url.clone(),
        llm_backend:    cfg.agent.backend.clone(),
        ws_connected:   Arc::clone(&ws_connected),
        contract_count: u32::try_from(contracts.len()).unwrap_or(u32::MAX),
    };

    // Spawn market data WebSocket task if contracts are configured
    if !contracts.is_empty() {
        let ws_flag = Arc::clone(&ws_connected);
        tokio::spawn(async move {
            run_market_data_ws(contracts, ws_flag).await;
        });
    }

    let addr = format!("0.0.0.0:{port}")
        .parse::<std::net::SocketAddr>()
        .map_err(|_| error::AppError::Config {
            message: format!("invalid port: {port}"),
        })?;

    eprintln!("gRPC server listening on {addr}");

    tonic::transport::Server::builder()
        .add_service(RaraServiceServer::new(RaraServiceImpl::with_health(
            health_config,
        )))
        .serve(addr)
        .await
        .context(GrpcServeSnafu)?;

    Ok(())
}

/// Maintain a market data WebSocket connection with automatic reconnection.
///
/// Sets `ws_flag` to `true` while connected and `false` on disconnect.
/// Retries with exponential backoff (1s → 60s cap) on failure.
async fn run_market_data_ws(contracts: Vec<String>, ws_flag: Arc<std::sync::atomic::AtomicBool>) {
    use std::sync::atomic::Ordering;

    use futures_util::StreamExt;
    use rara_market_data::stream::BinanceWsClient;

    let client = BinanceWsClient::new();
    let subs: Vec<(String, String)> = contracts
        .iter()
        .map(|c| (c.clone(), "1m".to_string()))
        .collect();

    let mut backoff = std::time::Duration::from_secs(1);
    let max_backoff = std::time::Duration::from_secs(60);

    loop {
        let sub_refs: Vec<(&str, &str)> =
            subs.iter().map(|(s, i)| (s.as_str(), i.as_str())).collect();

        match client.subscribe_klines_multi(&sub_refs).await {
            Ok(mut stream) => {
                tracing::info!(contracts = ?contracts, "market data WebSocket connected");
                ws_flag.store(true, Ordering::Relaxed);
                backoff = std::time::Duration::from_secs(1);

                // Drain the stream to keep the connection alive
                while let Some(item) = stream.next().await {
                    if let Err(e) = item {
                        tracing::warn!(error = %e, "market data stream error");
                        break;
                    }
                }

                tracing::warn!("market data WebSocket disconnected");
                ws_flag.store(false, Ordering::Relaxed);
            }
            Err(e) => {
                tracing::warn!(error = %e, backoff_secs = backoff.as_secs(), "market data WebSocket connection failed, retrying");
            }
        }

        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(max_backoff);
    }
}

/// Execute the paper trading subcommand.
async fn run_paper(action: PaperAction) -> error::Result<()> {
    match action {
        PaperAction::Start => run_paper_start().await,
        PaperAction::Status => run_paper_status(),
        PaperAction::Stop => {
            run_paper_stop();
            Ok(())
        }
    }
}

/// Show paper trading status by reading trading events from the event bus.
///
/// Aggregates order-submitted, order-filled, and order-rejected events by
/// strategy and prints a summary table to stderr plus JSON to stdout.
fn run_paper_status() -> error::Result<()> {
    use std::collections::BTreeMap;

    use rara_domain::event::EventType;

    let trace_path = paths::data_dir().join("trace");
    let event_bus_path = trace_path.join("events");

    if !event_bus_path.exists() {
        eprintln!("No event bus data found. Has paper trading been run?");
        println!(
            "{}",
            serde_json::to_string(&PaperStatusResponse {
                ok:           true,
                action:       "paper.status",
                strategies:   vec![],
                total_trades: 0,
            })
            .expect("PaperStatusResponse must serialize")
        );
        return Ok(());
    }

    let event_bus = EventBus::open(&event_bus_path).context(EventBusSnafu)?;
    let events = event_bus
        .store()
        .read_topic("trading", 0, 100_000)
        .context(EventBusSnafu)?;

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

/// Show instructions for stopping paper trading.
///
/// Paper trading runs in the foreground, so the user should press Ctrl+C in
/// the terminal where `paper start` is running. This command simply prints
/// that guidance.
fn run_paper_stop() {
    let message = "Paper trading runs in the foreground. Press Ctrl+C in the terminal where it's \
                   running."
        .to_string();
    eprintln!("{message}");
    println!(
        "{}",
        serde_json::to_string(&PaperStopResponse {
            ok: true,
            action: "paper.stop",
            message,
        })
        .expect("PaperStopResponse must serialize")
    );
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
/// Ctrl+C is received, then gracefully shuts down all tasks and prints a
/// session summary.
#[allow(clippy::too_many_lines)]
async fn run_paper_start() -> error::Result<()> {
    use futures_util::StreamExt;
    use rara_market_data::stream::{aggregator::CandleAggregator, binance_ws::BinanceWsClient};
    use rara_trading::trading::{
        engine::TradingEngine, guard_pipeline::GuardPipeline, signal_loop::run_signal_loop,
    };

    let cfg = app_config::load();

    // Load accounts from config; use AccountManager to create brokers
    let accounts_cfg = accounts_config::load_accounts();
    let account_manager =
        rara_trading_engine::account_manager::AccountManager::from_config(&accounts_cfg.accounts)
            .expect("failed to initialize accounts from config");

    if account_manager.size() == 0 {
        eprintln!(
            "No enabled accounts found in accounts.toml. Run 'rara setup account add' first."
        );
        return Ok(());
    }

    // Collect contracts from all enabled accounts
    let contracts: Vec<String> = accounts_cfg
        .accounts
        .iter()
        .filter(|a| a.enabled)
        .flat_map(|a| a.contracts.clone())
        .collect();

    let position_size =
        rust_decimal::Decimal::try_from(cfg.trading.max_position_size).unwrap_or(dec!(1));
    let loaded_strategies = load_strategies_for_contracts(&contracts, position_size)?;

    if loaded_strategies.is_empty() {
        eprintln!("No promoted strategies found. Run 'rara research run' first.");
        return Ok(());
    }

    let strategy_count = loaded_strategies.len();
    eprintln!(
        "Loaded {} strategy instances for {} contracts",
        strategy_count,
        contracts.len()
    );

    // Open event bus + build trading engine using the first account's broker
    // TODO: support multi-account engine routing once TradingEngine supports it
    let trace_path = paths::data_dir().join("trace");
    let event_bus = Arc::new(EventBus::open(&trace_path.join("events")).context(EventBusSnafu)?);
    let first_account = accounts_cfg
        .accounts
        .iter()
        .find(|a| a.enabled)
        .expect("at least one enabled account verified above");
    let broker = {
        let fields = first_account.broker_config.to_field_map();
        let type_key = first_account.broker_config.type_key();
        let entry = rara_trading_engine::broker_registry::find_broker(type_key)
            .ok_or_else(
                || rara_trading_engine::broker_registry::BrokerRegistryError::UnknownType {
                    type_key: type_key.to_string(),
                },
            )
            .context(error::BrokerRegistrySnafu)?;
        (entry.create_broker)(&fields).context(error::BrokerRegistrySnafu)?
    };
    let guard_pipeline = GuardPipeline::new(vec![]);
    let engine = Arc::new(TradingEngine::new(
        broker,
        guard_pipeline,
        Arc::clone(&event_bus),
    ));

    // Shutdown signal via watch channel — tasks check this to drain gracefully
    let (shutdown_tx, mut shutdown_rx_agg) = tokio::sync::watch::channel(false);
    let mut shutdown_rx_sig = shutdown_tx.subscribe();

    // Setup candle aggregator + WebSocket connection
    let (mut aggregator, candle_rx) = CandleAggregator::with_defaults();
    let ws_client = BinanceWsClient::new();
    let subs: Vec<(&str, &str)> = contracts.iter().map(|c| (c.as_str(), "1m")).collect();
    let mut kline_stream =
        ws_client
            .subscribe_klines_multi(&subs)
            .await
            .map_err(|e| error::AppError::Config {
                message: format!("WebSocket connection failed: {e}"),
            })?;

    // Spawn aggregator: forward raw klines into candle aggregator, exit on shutdown
    let agg_handle = tokio::spawn(async move {
        loop {
            tokio::select! {
                biased;
                _ = shutdown_rx_agg.changed() => {
                    tracing::info!("aggregator received shutdown signal");
                    break;
                }
                item = kline_stream.next() => {
                    match item {
                        Some(Ok(kline)) => aggregator.process_kline(&kline),
                        Some(Err(e)) => {
                            tracing::error!(error = %e, "kline stream error");
                            break;
                        }
                        None => {
                            tracing::info!("kline stream ended");
                            break;
                        }
                    }
                }
            }
        }
    });

    // Spawn signal loop: exits when the candle broadcast channel closes
    let signal_handle = tokio::spawn(async move {
        // Wait for either shutdown signal or natural loop end
        tokio::select! {
            biased;
            _ = shutdown_rx_sig.changed() => {
                tracing::info!("signal loop received shutdown signal");
            }
            () = run_signal_loop(candle_rx, engine, loaded_strategies) => {}
        }
    });

    let start_time = std::time::Instant::now();
    eprintln!("Paper trading started. Press Ctrl+C to stop.");

    tokio::signal::ctrl_c()
        .await
        .expect("failed to listen for ctrl+c");
    eprintln!("\nShutting down gracefully...");

    // Signal all tasks to stop
    let _ = shutdown_tx.send(true);

    // Wait for tasks to drain with a timeout
    let drain_timeout = std::time::Duration::from_secs(5);
    let _ = tokio::time::timeout(drain_timeout, async {
        let _ = agg_handle.await;
        let _ = signal_handle.await;
    })
    .await;

    let duration = start_time.elapsed();
    let duration_secs = duration.as_secs();
    let hours = duration_secs / 3600;
    let minutes = (duration_secs % 3600) / 60;

    // Count trades from event bus for the summary
    let trade_count = event_bus
        .store()
        .read_topic("trading", 0, 100_000)
        .map(|events| {
            events
                .iter()
                .filter(|e| e.event_type == rara_domain::event::EventType::TradingOrderFilled)
                .count()
        })
        .unwrap_or(0);

    eprintln!("=== Paper Trading Summary ===");
    eprintln!("Duration: {hours}h {minutes}m");
    eprintln!("Strategies: {strategy_count}");
    eprintln!("Total Trades: {trade_count}");

    println!(
        "{}",
        serde_json::to_string(&PaperShutdownSummary {
            ok: true,
            action: "paper.start",
            duration_secs,
            total_trades: trade_count,
        })
        .expect("PaperShutdownSummary must serialize")
    );

    eprintln!("Paper trading stopped.");
    Ok(())
}

/// Read promoted strategy metadata files from a directory without requiring
/// the full `StrategyPromoter` (which needs trace, runtime, and compiler).
fn list_promoted_from_dir(
    dir: &Path,
) -> rara_trading::research::strategy_promoter::Result<Vec<PromotedStrategy>> {
    use rara_trading::research::strategy_promoter::{
        IoSnafu as PmIoSnafu, SerializeSnafu as PmSerializeSnafu,
    };

    if !dir.exists() {
        return Ok(vec![]);
    }

    let mut promoted = Vec::new();
    let entries = std::fs::read_dir(dir).context(PmIoSnafu)?;

    for entry in entries {
        let entry = entry.context(PmIoSnafu)?;
        let path = entry.path();

        if path.extension().is_some_and(|ext| ext == "json")
            && !path
                .file_name()
                .is_some_and(|n| n.to_string_lossy().ends_with(".registry.json"))
        {
            let contents = std::fs::read_to_string(&path).context(PmIoSnafu)?;
            let strategy: PromotedStrategy =
                serde_json::from_str(&contents).context(PmSerializeSnafu)?;
            promoted.push(strategy);
        }
    }

    Ok(promoted)
}

// ---------------------------------------------------------------------------
// Strategy registry commands
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct StrategyListResponse {
    ok:         bool,
    action:     &'static str,
    strategies: Vec<StrategyListItem>,
}

#[derive(Serialize)]
struct StrategyListItem {
    name:    String,
    version: String,
    tag:     String,
    size:    u64,
}

#[derive(Serialize)]
struct StrategyFetchResponse {
    ok:          bool,
    action:      &'static str,
    name:        String,
    version:     String,
    api_version: u32,
    wasm_path:   String,
}

#[derive(Serialize)]
struct StrategyInstalledResponse {
    ok:         bool,
    action:     &'static str,
    strategies: Vec<StrategyInstalledItem>,
}

#[derive(Serialize)]
struct StrategyInstalledItem {
    name:        String,
    version:     String,
    api_version: u32,
    wasm_path:   String,
}

async fn run_strategy(action: StrategyAction) -> error::Result<()> {
    match action {
        StrategyAction::List { repo } => run_strategy_list(&repo).await,
        StrategyAction::Fetch { name, repo } => run_strategy_fetch(&name, &repo).await,
        StrategyAction::Installed => run_strategy_installed(),
        StrategyAction::Backtest {
            name,
            contract,
            timeframe,
        } => run_strategy_backtest(&name, &contract, &timeframe).await,
    }
}

async fn run_strategy_list(repo: &str) -> error::Result<()> {
    let registry = rara_trading::research::strategy_registry::StrategyRegistry::builder()
        .repo(repo.to_string())
        .promoted_dir(paths::strategies_promoted_dir())
        .build();

    let entries = registry.list_available().await.context(RegistrySnafu)?;

    let items: Vec<StrategyListItem> = entries
        .iter()
        .map(|e| StrategyListItem {
            name:    e.name.clone(),
            version: e.version.clone(),
            tag:     e.tag.clone(),
            size:    e.size,
        })
        .collect();

    println!(
        "{}",
        serde_json::to_string(&StrategyListResponse {
            ok:         true,
            action:     "strategy.list",
            strategies: items,
        })
        .expect("StrategyListResponse must serialize")
    );

    Ok(())
}

async fn run_strategy_fetch(name: &str, repo: &str) -> error::Result<()> {
    let promoted_dir = paths::strategies_promoted_dir();
    std::fs::create_dir_all(&promoted_dir).context(IoSnafu)?;

    let registry = rara_trading::research::strategy_registry::StrategyRegistry::builder()
        .repo(repo.to_string())
        .promoted_dir(promoted_dir)
        .build();

    let fetched = registry.fetch(name).await.context(RegistrySnafu)?;

    println!(
        "{}",
        serde_json::to_string(&StrategyFetchResponse {
            ok:          true,
            action:      "strategy.fetch",
            name:        fetched.meta.name,
            version:     format!("v{}", fetched.meta.version),
            api_version: fetched.meta.api_version,
            wasm_path:   fetched.wasm_path.display().to_string(),
        })
        .expect("StrategyFetchResponse must serialize")
    );

    Ok(())
}

fn run_strategy_installed() -> error::Result<()> {
    let registry = rara_trading::research::strategy_registry::StrategyRegistry::builder()
        .promoted_dir(paths::strategies_promoted_dir())
        .build();

    let installed = registry.list_installed().context(RegistrySnafu)?;

    let items: Vec<StrategyInstalledItem> = installed
        .iter()
        .map(|s| StrategyInstalledItem {
            name:        s.meta.name.clone(),
            version:     format!("v{}", s.meta.version),
            api_version: s.meta.api_version,
            wasm_path:   s.wasm_path.display().to_string(),
        })
        .collect();

    println!(
        "{}",
        serde_json::to_string(&StrategyInstalledResponse {
            ok:         true,
            action:     "strategy.installed",
            strategies: items,
        })
        .expect("StrategyInstalledResponse must serialize")
    );

    Ok(())
}

#[derive(Serialize)]
struct BacktestResponse {
    ok:           bool,
    action:       &'static str,
    strategy:     String,
    contract:     String,
    timeframe:    String,
    pnl:          String,
    sharpe_ratio: f64,
    max_drawdown: String,
    win_rate:     f64,
    trade_count:  u32,
}

/// Run a backtest on a fetched WASM strategy against historical market data.
async fn run_strategy_backtest(
    name: &str,
    contract: &str,
    timeframe_str: &str,
) -> error::Result<()> {
    use rara_trading::research::{
        backtester::Backtester, barter_backtester::BarterBacktester, wasm_executor::WasmExecutor,
    };
    use rust_decimal_macros::dec;
    use tracing::info;

    let timeframe: rara_domain::timeframe::Timeframe =
        timeframe_str.parse().map_err(|_| error::AppError::Config {
            message: format!("invalid timeframe: {timeframe_str}"),
        })?;

    // Load WASM from promoted directory
    let wasm_path = paths::strategies_promoted_dir().join(format!("{name}.wasm"));
    if !wasm_path.exists() {
        return Err(error::AppError::Config {
            message: format!(
                "strategy '{name}' not found at {}. Run `rara strategy fetch {name}` first.",
                wasm_path.display()
            ),
        });
    }

    let wasm_bytes = std::fs::read(&wasm_path).map_err(|e| error::AppError::Config {
        message: format!("failed to read {}: {e}", wasm_path.display()),
    })?;

    let executor = WasmExecutor::builder().build();
    let handle = executor
        .load(&wasm_bytes)
        .map_err(|e| error::AppError::Config {
            message: format!("failed to load WASM module: {e}"),
        })?;

    let meta = {
        let mut h = executor
            .load(&wasm_bytes)
            .map_err(|e| error::AppError::Config {
                message: format!("failed to load WASM for metadata: {e}"),
            })?;
        h.meta().map_err(|e| error::AppError::Config {
            message: format!("failed to read strategy metadata: {e}"),
        })?
    };

    info!(
        strategy = meta.name,
        version = meta.version,
        contract,
        %timeframe,
        "starting backtest"
    );

    // Build backtester
    let cfg = app_config::load();
    let market_store = rara_market_data::store::MarketStore::connect(&cfg.database.url)
        .await
        .context(MarketStoreSnafu)?;

    let backtester = BarterBacktester::builder()
        .store(market_store)
        .initial_capital(dec!(10000))
        .fees_percent(dec!(0.1))
        .backtest_start(chrono::NaiveDate::from_ymd_opt(2020, 1, 1).expect("valid date"))
        .backtest_end(chrono::NaiveDate::from_ymd_opt(2030, 12, 31).expect("valid date"))
        .build();

    let result = backtester
        .run(handle, contract, timeframe)
        .await
        .map_err(|e| error::AppError::Config {
            message: format!("backtest failed: {e}"),
        })?;

    println!(
        "{}",
        serde_json::to_string(&BacktestResponse {
            ok:           true,
            action:       "strategy.backtest",
            strategy:     meta.name,
            contract:     contract.to_string(),
            timeframe:    timeframe_str.to_string(),
            pnl:          result.pnl.to_string(),
            sharpe_ratio: result.sharpe_ratio,
            max_drawdown: result.max_drawdown.to_string(),
            win_rate:     result.win_rate,
            trade_count:  result.trade_count,
        })
        .expect("BacktestResponse must serialize")
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// Setup command handlers
// ---------------------------------------------------------------------------

async fn run_setup(action: SetupAction) -> error::Result<()> {
    match action {
        SetupAction::Init { force } => run_setup_init(force)?,
        SetupAction::Validate => run_setup_validate().await?,
        SetupAction::Account { action } => run_setup_account(*action).await?,
        SetupAction::Data {
            source,
            search,
            start,
            end,
            symbols,
        } => run_setup_data(&source, search, start, end, symbols).await?,
    }
    Ok(())
}

/// Download historical market data for backtesting.
///
/// With `--search`, queries Binance for matching symbols. Otherwise downloads
/// the given symbols (defaulting to BTCUSDT + ETHUSDT). When `--start` is
/// omitted, auto-detects the earliest available date per symbol.
async fn run_setup_data(
    source: &str,
    search: Option<String>,
    start: Option<String>,
    end: Option<String>,
    mut symbols: Vec<String>,
) -> error::Result<()> {
    let parse_date = |s: &str, label: &str| -> error::Result<NaiveDate> {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").map_err(|_| AppError::Config {
            message: format!("invalid {label} date: {s}"),
        })
    };

    let start = start
        .as_deref()
        .map(|s| parse_date(s, "start"))
        .transpose()?;
    let end = end.as_deref().map(|s| parse_date(s, "end")).transpose()?;

    // Symbol search mode (Binance only)
    if let Some(query) = search {
        eprintln!("Searching Binance for \"{query}\"…");
        let results = rara_market_data::fetcher::binance::search_symbols(&query)
            .await
            .context(DataFetchSnafu)?;

        if results.is_empty() {
            eprintln!("No USDT spot symbols found matching \"{query}\".");
            return Ok(());
        }

        for s in &results {
            eprintln!("  {s}");
        }
        eprintln!("{} symbols found.", results.len());

        if symbols.is_empty() {
            return Ok(());
        }
    }

    // Default symbols
    if symbols.is_empty() {
        symbols = vec!["BTCUSDT".to_string(), "ETHUSDT".to_string()];
    }

    let cfg = app_config::load();
    rara_trading::setup_wizard::download_symbols_parallel(
        &cfg.database.url,
        source,
        &symbols,
        start,
        end,
    )
    .await
}

/// Generate config.toml and accounts.toml templates.
fn run_setup_init(force: bool) -> error::Result<()> {
    let config_path = paths::config_file();
    let accounts_path = paths::accounts_file();

    let mut created = Vec::new();

    // Ensure parent directories exist
    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent).context(IoSnafu)?;
    }
    if let Some(parent) = accounts_path.parent() {
        std::fs::create_dir_all(parent).context(IoSnafu)?;
    }

    let config_exists = config_path.exists();
    let accounts_exists = accounts_path.exists();

    if !config_exists || force {
        let template = app_config::generate_template();
        std::fs::write(&config_path, &template).context(IoSnafu)?;
        eprintln!("wrote {}", config_path.display());
        created.push("config.toml".to_string());
    }

    if !accounts_exists || force {
        let template = accounts_config::generate_accounts_template();
        std::fs::write(&accounts_path, &template).context(IoSnafu)?;
        eprintln!("wrote {}", accounts_path.display());
        created.push("accounts.toml".to_string());
    }

    let reason = if created.is_empty() {
        Some("files already exist".to_string())
    } else {
        None
    };

    println!(
        "{}",
        serde_json::to_string(&SetupInitResponse {
            ok: true,
            action: "init",
            created,
            reason,
        })
        .expect("SetupInitResponse must serialize")
    );

    Ok(())
}

/// Validate all configuration files and connectivity.
#[allow(clippy::too_many_lines)]
async fn run_setup_validate() -> error::Result<()> {
    let mut checks = Vec::new();
    let mut has_errors = false;

    // Check config.toml exists
    let config_path = paths::config_file();
    let config_exists = config_path.exists();
    checks.push(ValidateCheck {
        name:       "config.toml".to_string(),
        ok:         config_exists,
        detail:     if config_exists {
            None
        } else {
            has_errors = true;
            Some(format!("not found at {}", config_path.display()))
        },
        suggestion: if config_exists {
            None
        } else {
            Some("run 'rara setup init' to generate config files".to_string())
        },
    });

    // Check accounts.toml exists
    let accounts_path = paths::accounts_file();
    let accounts_exists = accounts_path.exists();
    checks.push(ValidateCheck {
        name:       "accounts.toml".to_string(),
        ok:         accounts_exists,
        detail:     if accounts_exists {
            None
        } else {
            has_errors = true;
            Some(format!("not found at {}", accounts_path.display()))
        },
        suggestion: if accounts_exists {
            None
        } else {
            Some("run 'rara setup init' to generate config files".to_string())
        },
    });

    // Check for duplicate account IDs
    let accounts_cfg = accounts_config::load_accounts();
    let mut seen_ids = std::collections::HashSet::new();
    let mut duplicates = Vec::new();
    for acc in &accounts_cfg.accounts {
        if !seen_ids.insert(&acc.id) {
            duplicates.push(acc.id.clone());
        }
    }
    let no_dupes = duplicates.is_empty();
    checks.push(ValidateCheck {
        name:       "unique_account_ids".to_string(),
        ok:         no_dupes,
        detail:     if no_dupes {
            None
        } else {
            has_errors = true;
            Some(format!("duplicate IDs: {}", duplicates.join(", ")))
        },
        suggestion: if no_dupes {
            None
        } else {
            Some("fix accounts.toml or run 'rara setup account add' to reconfigure".to_string())
        },
    });

    // Count enabled accounts
    let enabled_count = accounts_cfg.accounts.iter().filter(|a| a.enabled).count();
    checks.push(ValidateCheck {
        name:       "enabled_accounts".to_string(),
        ok:         true,
        detail:     Some(format!("{enabled_count} account(s) enabled")),
        suggestion: None,
    });

    // Run startup validation (database + LLM connectivity)
    if config_exists {
        let cfg = app_config::load();
        let startup_errors = validation::validate_startup(cfg).await;
        for e in &startup_errors {
            has_errors = true;
            checks.push(ValidateCheck {
                name:       "startup".to_string(),
                ok:         false,
                detail:     Some(e.to_string()),
                suggestion: None,
            });
        }
        if startup_errors.is_empty() {
            checks.push(ValidateCheck {
                name:       "startup".to_string(),
                ok:         true,
                detail:     None,
                suggestion: None,
            });
        }
    }

    for check in &checks {
        if check.ok {
            eprintln!("OK: {}", check.name);
        } else {
            eprintln!(
                "FAIL: {} — {}",
                check.name,
                check.detail.as_deref().unwrap_or("")
            );
        }
    }

    println!(
        "{}",
        serde_json::to_string(&SetupValidateResponse {
            ok: !has_errors,
            action: "validate",
            checks,
        })
        .expect("SetupValidateResponse must serialize")
    );

    if has_errors {
        std::process::exit(1);
    }

    Ok(())
}

/// Handle account subcommands.
#[allow(clippy::too_many_lines)]
async fn run_setup_account(action: SetupAccountAction) -> error::Result<()> {
    use rara_trading_engine::account_config::{
        AccountConfig, BrokerConfig, CcxtBrokerConfig, PaperBrokerConfig,
    };

    match action {
        SetupAccountAction::Add {
            id,
            broker,
            label,
            contracts,
            enabled,
            fill_price,
            exchange,
            api_key,
            secret,
            passphrase,
            sandbox,
        } => {
            let mut cfg = accounts_config::load_accounts();

            // Idempotent: if account already exists, return created: false
            if cfg.accounts.iter().any(|a| a.id == id) {
                println!(
                    "{}",
                    serde_json::to_string(&SetupAccountAddResponse {
                        ok:      true,
                        action:  "account.add",
                        id:      &id,
                        created: false,
                    })
                    .expect("SetupAccountAddResponse must serialize")
                );
                return Ok(());
            }

            let broker_config = match broker.as_str() {
                "paper" => BrokerConfig::Paper(PaperBrokerConfig { fill_price }),
                "ccxt" => {
                    let Some(exchange) = exchange else {
                        println!(
                            "{}",
                            serde_json::to_string(&ErrorResponse {
                                ok:         false,
                                error:      "--exchange is required for ccxt broker".to_string(),
                                suggestion: Some(
                                    "add --exchange binance (or bybit, okx)".to_string()
                                ),
                            })
                            .expect("ErrorResponse must serialize")
                        );
                        std::process::exit(1);
                    };
                    BrokerConfig::Ccxt(CcxtBrokerConfig {
                        exchange,
                        sandbox,
                        api_key: api_key.unwrap_or_default(),
                        secret: secret.unwrap_or_default(),
                        passphrase,
                    })
                }
                other => {
                    println!(
                        "{}",
                        serde_json::to_string(&ErrorResponse {
                            ok:         false,
                            error:      format!("unknown broker type \"{other}\""),
                            suggestion: Some("use --broker paper or --broker ccxt".to_string()),
                        })
                        .expect("ErrorResponse must serialize")
                    );
                    std::process::exit(1);
                }
            };

            cfg.accounts.push(AccountConfig {
                id: id.clone(),
                label,
                broker_config,
                enabled,
                contracts: contracts.unwrap_or_default(),
            });

            accounts_config::save_accounts(&cfg).context(IoSnafu)?;
            eprintln!("account \"{id}\" added");

            println!(
                "{}",
                serde_json::to_string(&SetupAccountAddResponse {
                    ok:      true,
                    action:  "account.add",
                    id:      &id,
                    created: true,
                })
                .expect("SetupAccountAddResponse must serialize")
            );
        }

        SetupAccountAction::List => {
            let mut cfg = accounts_config::load_accounts();
            for acc in &mut cfg.accounts {
                acc.mask_secrets();
            }
            let accounts: Vec<serde_json::Value> = cfg
                .accounts
                .iter()
                .map(|a| serde_json::to_value(a).expect("AccountConfig must serialize"))
                .collect();

            println!(
                "{}",
                serde_json::to_string(&SetupAccountListResponse {
                    ok: true,
                    action: "account.list",
                    accounts,
                })
                .expect("SetupAccountListResponse must serialize")
            );
        }

        SetupAccountAction::Remove { id, yes } => {
            if !yes {
                println!(
                    "{}",
                    serde_json::to_string(&ErrorResponse {
                        ok:         false,
                        error:      "--yes flag is required to confirm removal".to_string(),
                        suggestion: Some("add --yes to confirm removal".to_string()),
                    })
                    .expect("ErrorResponse must serialize")
                );
                std::process::exit(1);
            }

            let mut cfg = accounts_config::load_accounts();
            let original_len = cfg.accounts.len();
            cfg.accounts.retain(|a| a.id != id);

            if cfg.accounts.len() == original_len {
                println!(
                    "{}",
                    serde_json::to_string(&ErrorResponse {
                        ok:         false,
                        error:      format!("account \"{id}\" not found"),
                        suggestion: Some(
                            "run 'rara setup account list' to see available accounts".to_string()
                        ),
                    })
                    .expect("ErrorResponse must serialize")
                );
                std::process::exit(1);
            }

            accounts_config::save_accounts(&cfg).context(IoSnafu)?;
            eprintln!("account \"{id}\" removed");

            println!(
                "{}",
                serde_json::to_string(&SetupAccountRemoveResponse {
                    ok:      true,
                    action:  "account.remove",
                    id:      &id,
                    removed: true,
                })
                .expect("SetupAccountRemoveResponse must serialize")
            );
        }

        SetupAccountAction::Test { id } => {
            let cfg = accounts_config::load_accounts();
            let Some(acc) = cfg.accounts.iter().find(|a| a.id == id) else {
                println!(
                    "{}",
                    serde_json::to_string(&ErrorResponse {
                        ok:         false,
                        error:      format!("account \"{id}\" not found"),
                        suggestion: Some(
                            "run 'rara setup account list' to see available accounts".to_string(),
                        ),
                    })
                    .expect("ErrorResponse must serialize")
                );
                std::process::exit(1);
            };

            let broker = {
                let fields = acc.broker_config.to_field_map();
                let type_key = acc.broker_config.type_key();
                let entry = rara_trading_engine::broker_registry::find_broker(type_key)
                    .ok_or_else(|| {
                        rara_trading_engine::broker_registry::BrokerRegistryError::UnknownType {
                            type_key: type_key.to_string(),
                        }
                    })
                    .context(error::BrokerRegistrySnafu)?;
                (entry.create_broker)(&fields).context(error::BrokerRegistrySnafu)?
            };
            match broker.account_info().await {
                Ok(info) => {
                    println!(
                        "{}",
                        serde_json::to_string(&SetupAccountTestResponse {
                            ok:             true,
                            action:         "account.test",
                            id:             &id,
                            equity:         info.total_equity.to_string(),
                            available_cash: info.available_cash.to_string(),
                        })
                        .expect("SetupAccountTestResponse must serialize")
                    );
                }
                Err(e) => {
                    println!(
                        "{}",
                        serde_json::to_string(&ErrorResponse {
                            ok:         false,
                            error:      format!("connectivity test failed: {e}"),
                            suggestion: Some(
                                "check API credentials and network connectivity".to_string(),
                            ),
                        })
                        .expect("ErrorResponse must serialize")
                    );
                    std::process::exit(1);
                }
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Events command handler
// ---------------------------------------------------------------------------

/// JSON response for a single event returned by the events query.
#[derive(Serialize)]
struct EventQueryItem {
    /// Sequence number in the event store.
    seq:            u64,
    /// Event identifier.
    event_id:       String,
    /// Event type.
    event_type:     String,
    /// Correlation ID.
    correlation_id: String,
    /// Source component.
    source:         String,
    /// ISO-8601 timestamp.
    timestamp:      String,
    /// Arbitrary event payload.
    payload:        serde_json::Value,
}

/// JSON response for the events query command.
#[derive(Serialize)]
struct EventQueryResponse {
    ok:             bool,
    correlation_id: String,
    count:          usize,
    events:         Vec<EventQueryItem>,
}

fn run_events(action: EventsAction) -> error::Result<()> {
    match action {
        EventsAction::Query {
            correlation_id,
            limit,
        } => {
            let bus = EventBus::open(&paths::event_bus_dir()).context(EventBusSnafu)?;
            let all_events = bus
                .read_by_correlation_id(&correlation_id)
                .context(EventBusSnafu)?;

            let events: Vec<EventQueryItem> = all_events
                .into_iter()
                .take(limit)
                .enumerate()
                .map(|(i, e)| EventQueryItem {
                    // seq is not directly exposed; use index as a stable surrogate
                    seq:            i as u64,
                    event_id:       e.event_id.to_string(),
                    event_type:     e.event_type.to_string(),
                    correlation_id: e.correlation_id.clone(),
                    source:         e.source.clone(),
                    timestamp:      e.timestamp.to_string(),
                    payload:        e.payload,
                })
                .collect();

            let resp = EventQueryResponse {
                ok: true,
                correlation_id,
                count: events.len(),
                events,
            };
            println!(
                "{}",
                serde_json::to_string_pretty(&resp).expect("response must serialize")
            );
            Ok(())
        }
    }
}
