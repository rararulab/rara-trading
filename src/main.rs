use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser;
use rust_decimal_macros::dec;
use snafu::ResultExt;

use rara_trading::agent::{CliBackend, CliExecutor};
use rara_trading::app_config;
use rara_trading::cli::{Cli, Command, ConfigAction, ResearchAction};
use rara_trading::error::{
    self, AgentBackendSnafu, AgentExecutionSnafu, ConfigSnafu, EventBusSnafu, IoSnafu,
    PromptRendererSnafu, TraceSnafu,
};
use rara_trading::event_bus::bus::EventBus;
use rara_trading::paths;
use rara_trading::research::barter_backtester::BarterBacktester;
use rara_trading::research::compiler::StrategyCompiler;
use rara_trading::research::feedback_gen::FeedbackGenerator;
use rara_trading::research::hypothesis_gen::HypothesisGenerator;
use rara_trading::research::prompt_renderer::PromptRenderer;
use rara_trading::research::research_loop::ResearchLoop;
use rara_trading::research::runtime::StrategyRuntime;
use rara_trading::research::strategy_coder::StrategyCoder;
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
            serde_json::json!({"ok": false, "error": e.to_string()})
        );
        std::process::exit(1);
    }
}

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
                    serde_json::json!({"ok": true, "action": "config_set", "key": key, "value": value})
                );
            }
            ConfigAction::Get { key } => {
                let cfg = app_config::load();
                let value = get_config_field(cfg, &key)?;
                let display_value = value.as_deref().unwrap_or("(not set)");
                println!(
                    "{}",
                    serde_json::json!({"ok": true, "action": "config_get", "key": key, "value": display_value})
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
                    serde_json::json!({"ok": true, "action": "config_list", "entries": map})
                );
            }
        },
        Command::Hello { name } => {
            let greeting = format!("Hello, {name}!");
            eprintln!("{greeting}");
            println!(
                "{}",
                serde_json::json!({"ok": true, "action": "hello", "greeting": greeting})
            );
        }
        Command::Research { action } => {
            run_research(action).await?;
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
                serde_json::json!({
                    "ok": result.success,
                    "action": "agent_run",
                    "exit_code": result.exit_code,
                    "timed_out": result.timed_out,
                    "output": result.output,
                })
            );
        }
    }

    Ok(())
}

/// Set a config field by dotted key path.
fn set_config_field(cfg: &mut app_config::AppConfig, key: &str, value: &str) -> error::Result<()> {
    match key {
        "example.setting" => cfg.example.setting = value.to_string(),
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
        "example.setting" => Ok(Some(cfg.example.setting.clone())),
        "agent.backend" => Ok(Some(cfg.agent.backend.clone())),
        "agent.command" => Ok(cfg.agent.command.clone()),
        "agent.idle_timeout_secs" => Ok(Some(cfg.agent.idle_timeout_secs.to_string())),
        _ => ConfigSnafu { message: format!("unknown config key: {key}") }.fail(),
    }
}

/// Flatten config into key-value pairs for listing.
fn config_as_map(cfg: &app_config::AppConfig) -> Vec<(String, String)> {
    vec![
        ("example.setting".to_string(), cfg.example.setting.clone()),
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

/// Execute the research subcommand.
async fn run_research(action: ResearchAction) -> error::Result<()> {
    match action {
        ResearchAction::Run {
            iterations,
            contract,
            trace_dir,
        } => {
            // 1. Set up paths
            let trace_path = trace_dir
                .map_or_else(|| paths::data_dir().join("trace"), PathBuf::from);
            let template_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("strategies/template");
            let prompts_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("crates/rara-research/src/prompts");

            // 2. Initialize components
            let trace = Trace::open(&trace_path).context(TraceSnafu)?;
            let event_bus = Arc::new(
                EventBus::open(&trace_path.join("events")).context(EventBusSnafu)?,
            );
            let compiler = StrategyCompiler::builder()
                .template_dir(template_dir)
                .build();
            let runtime = StrategyRuntime::builder().build();
            let prompt_renderer =
                PromptRenderer::load_from_dir(&prompts_dir).context(PromptRendererSnafu)?;
            let prompt_renderer_for_loop =
                PromptRenderer::load_from_dir(&prompts_dir).context(PromptRendererSnafu)?;
            let backtester = BarterBacktester::builder()
                .data_dir(paths::data_dir().join("market_data"))
                .initial_capital(dec!(10000))
                .fees_percent(dec!(0.1))
                .build();

            // 3. Initialize LLM via configured agent backend
            let cfg = app_config::load();
            let cli_backend =
                CliBackend::from_agent_config(&cfg.agent).context(error::AgentBackendSnafu)?;
            let llm = CliExecutor::new(cli_backend);

            // 4. Build sub-components
            let feedback_gen = FeedbackGenerator::new(llm.clone(), prompt_renderer);
            let hypothesis_gen = HypothesisGenerator::new(llm.clone());
            let strategy_coder = StrategyCoder::new(llm);

            // 5. Build ResearchLoop
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
                .build();

            // 6. Run iterations
            for i in 1..=iterations {
                eprintln!("[iteration {i}/{iterations}] running...");
                let result = research_loop.run_iteration(&contract).await;
                match result {
                    Ok(ir) => {
                        let status = if ir.accepted { "ACCEPTED" } else { "rejected" };
                        eprintln!(
                            "[iteration {i}/{iterations}] {status} — hypothesis: {}",
                            ir.hypothesis.text()
                        );
                        println!(
                            "{}",
                            serde_json::json!({
                                "iteration": i,
                                "accepted": ir.accepted,
                                "hypothesis": ir.hypothesis.text(),
                            })
                        );
                    }
                    Err(e) => {
                        eprintln!("[iteration {i}/{iterations}] ERROR: {e}");
                    }
                }
            }

            println!(
                "{}",
                serde_json::json!({"ok": true, "action": "research.run", "iterations": iterations})
            );
        }
    }
    Ok(())
}
