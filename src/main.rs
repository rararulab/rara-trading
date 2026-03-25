use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::NaiveDate;
use clap::Parser;
use rust_decimal_macros::dec;
use serde::Serialize;
use snafu::ResultExt;

use rara_trading::agent::{CliBackend, CliExecutor};
use rara_trading::app_config;
use rara_trading::cli::{Cli, Command, ConfigAction, DataAction, ResearchAction};
use rara_trading::error::{
    self, AgentBackendSnafu, AgentExecutionSnafu, ConfigSnafu, DataFetchSnafu, EventBusSnafu,
    IoSnafu, MarketStoreSnafu, PromoterSnafu, PromptRendererSnafu, TraceSnafu,
};
use rara_trading::event_bus::bus::EventBus;
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

use rara_trading::research::barter_backtester::BarterBacktester;
use rara_trading::research::compiler::StrategyCompiler;
use rara_trading::research::feedback_gen::FeedbackGenerator;
use rara_trading::research::hypothesis_gen::HypothesisGenerator;
use rara_trading::research::prompt_renderer::PromptRenderer;
use rara_trading::research::research_loop::ResearchLoop;
use rara_trading::research::runtime::StrategyRuntime;
use rara_trading::research::strategy_coder::StrategyCoder;
use rara_trading::research::strategy_promoter::PromotedStrategy;
use rara_trading::research::trace::Trace;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::WARN.into()),
        )
        .init();

    if let Err(e) = run().await {
        eprintln!("Error: {e}");
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
            ConfigAction::Set { key, value } => {
                let mut cfg = app_config::load().clone();
                set_config_field(&mut cfg, &key, &value)?;
                app_config::save(&cfg).context(IoSnafu)?;
                eprintln!("set {key} = {value}");
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
            eprintln!("{greeting}");
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
    match key {
        "agent.backend" => cfg.agent.backend = value.to_string(),
        "agent.command" => cfg.agent.command = Some(value.to_string()),
        "agent.idle_timeout_secs" => {
            cfg.agent.idle_timeout_secs = value.parse().map_err(|_| {
                error::AppError::Config {
                    message: format!("invalid integer for {key}: {value}"),
                }
            })?;
        }
        _ => return ConfigSnafu { message: format!("unknown config key: {key}") }.fail(),
    }
    Ok(())
}

/// Get a config field by dotted key path.
fn get_config_field(cfg: &app_config::AppConfig, key: &str) -> error::Result<Option<String>> {
    match key {
        "agent.backend" => Ok(Some(cfg.agent.backend.clone())),
        "agent.command" => Ok(cfg.agent.command.clone()),
        "agent.idle_timeout_secs" => Ok(Some(cfg.agent.idle_timeout_secs.to_string())),
        _ => ConfigSnafu { message: format!("unknown config key: {key}") }.fail(),
    }
}

/// Flatten config into key-value pairs for listing.
fn config_as_map(cfg: &app_config::AppConfig) -> Vec<(String, String)> {
    vec![
        ("agent.backend".to_string(), cfg.agent.backend.clone()),
        (
            "agent.command".to_string(),
            cfg.agent
                .command
                .as_deref()
                .unwrap_or("(not set)")
                .to_string(),
        ),
        (
            "agent.idle_timeout_secs".to_string(),
            cfg.agent.idle_timeout_secs.to_string(),
        ),
    ]
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

    eprintln!("fetched {count} candles for {instrument_id} from {source}");
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
        } => run_research_loop(iterations, &contract, trace_dir).await,
        ResearchAction::List { limit, trace_dir } => run_research_list(limit, trace_dir),
        ResearchAction::Show {
            experiment_id,
            trace_dir,
        } => run_research_show(&experiment_id, trace_dir),
        ResearchAction::Promoted { promoted_dir } => run_research_promoted(promoted_dir),
    }
}

/// Run N iterations of the research loop.
async fn run_research_loop(
    iterations: u32,
    contract: &str,
    trace_dir: Option<String>,
) -> error::Result<()> {
    let trace_path = trace_dir.map_or_else(|| paths::data_dir().join("trace"), PathBuf::from);
    let template_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("strategies/template");
    let prompts_dir =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("crates/rara-research/src/prompts");

    let trace = Trace::open(&trace_path).context(TraceSnafu)?;
    let event_bus = Arc::new(EventBus::open(&trace_path.join("events")).context(EventBusSnafu)?);
    let compiler = StrategyCompiler::builder()
        .template_dir(template_dir)
        .build();
    let runtime = StrategyRuntime::builder().build();
    let prompt_renderer =
        PromptRenderer::load_from_dir(&prompts_dir).context(PromptRendererSnafu)?;
    let prompt_renderer_for_loop =
        PromptRenderer::load_from_dir(&prompts_dir).context(PromptRendererSnafu)?;
    let cfg = app_config::load();
    let store = rara_market_data::store::MarketStore::connect(&cfg.database.url)
        .await
        .context(MarketStoreSnafu)?;

    let backtester = BarterBacktester::builder()
        .store(store)
        .initial_capital(dec!(10000))
        .fees_percent(dec!(0.1))
        .backtest_start(NaiveDate::from_ymd_opt(2020, 1, 1).expect("valid date"))
        .backtest_end(NaiveDate::from_ymd_opt(2030, 12, 31).expect("valid date"))
        .build();

    let cfg = app_config::load();
    let cli_backend =
        CliBackend::from_agent_config(&cfg.agent).context(error::AgentBackendSnafu)?;
    let llm = CliExecutor::new(cli_backend);

    let feedback_gen = FeedbackGenerator::new(llm.clone(), prompt_renderer);
    let hypothesis_gen = HypothesisGenerator::new(llm.clone());
    let strategy_coder = StrategyCoder::new(llm);

    let research_loop = ResearchLoop::builder()
        .hypothesis_gen(hypothesis_gen)
        .strategy_coder(strategy_coder)
        .compiler(compiler)
        .runtime(runtime)
        .backtester(backtester)
        .feedback_gen(feedback_gen)
        .prompt_renderer(prompt_renderer_for_loop)
        .trace(trace)
        .event_bus(event_bus)
        .generated_dir(paths::strategies_generated_dir())
        .promoted_dir(paths::strategies_promoted_dir())
        .build();

    for i in 1..=iterations {
        eprintln!("[iteration {i}/{iterations}] running...");
        let result = research_loop.run_iteration(contract).await;
        match result {
            Ok(ir) => {
                let status = if ir.accepted { "ACCEPTED" } else { "rejected" };
                eprintln!(
                    "[iteration {i}/{iterations}] {status} — hypothesis: {}",
                    ir.hypothesis.text
                );
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
                eprintln!("[iteration {i}/{iterations}] ERROR: {e}");
            }
        }
    }

    println!(
        "{}",
        serde_json::to_string(&ResearchRunResponse {
            ok: true,
            action: "research.run",
            iterations,
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
