//! Interactive setup wizard — detects prerequisites, guides configuration,
//! and validates the result so first-time users can get running painlessly.

use dialoguer::{Confirm, Input, Password, Select};
use snafu::ResultExt;

use crate::accounts_config;
use crate::app_config::{self, AppConfig};
use crate::error::{self, IoSnafu};
use crate::paths;
use crate::validation;

use rara_trading_engine::account_config::{
    AccountConfig, BrokerConfig, CcxtBrokerConfig, PaperBrokerConfig,
};

// ---------------------------------------------------------------------------
// Dialoguer helpers
// ---------------------------------------------------------------------------

/// Convert `dialoguer::Error` (which wraps `std::io::Error`) into `std::io::Error`
/// so it can be used with `IoSnafu`.
fn dialog_io(e: dialoguer::Error) -> std::io::Error {
    match e {
        dialoguer::Error::IO(io) => io,
    }
}

/// Helper to run a dialoguer `Confirm` prompt with consistent error handling.
fn confirm(prompt: &str, default: bool) -> error::Result<bool> {
    Confirm::new()
        .with_prompt(prompt)
        .default(default)
        .interact()
        .map_err(dialog_io)
        .context(IoSnafu)
}

/// Helper to run a dialoguer `Input<String>` prompt with consistent error handling.
fn input(prompt: &str, default: Option<&str>) -> error::Result<String> {
    let builder = Input::new().with_prompt(prompt);
    let builder = match default {
        Some(d) => builder.default(d.to_string()),
        None => builder,
    };
    builder
        .interact_text()
        .map_err(dialog_io)
        .context(IoSnafu)
}

/// Helper to run a dialoguer `Password` prompt with consistent error handling.
fn password(prompt: &str) -> error::Result<String> {
    Password::new()
        .with_prompt(prompt)
        .allow_empty_password(true)
        .interact()
        .map_err(dialog_io)
        .context(IoSnafu)
}

/// Helper to run a dialoguer `Select` prompt with consistent error handling.
fn select(prompt: &str, items: &[&str], default: usize) -> error::Result<usize> {
    Select::new()
        .with_prompt(prompt)
        .items(items)
        .default(default)
        .interact()
        .map_err(dialog_io)
        .context(IoSnafu)
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Interactive guided setup — detects, configures, and validates.
///
/// The wizard checks what's already working, only asks about things that need
/// attention, and gives actionable guidance when something is missing.
///
/// Requires a TTY on stdin; returns an error if prompts cannot be displayed.
pub async fn run() -> error::Result<()> {
    print_welcome();

    let cfg = step_database().await?;
    let cfg = step_llm_backend(cfg)?;
    let accounts = step_accounts()?;

    print_summary(&cfg, &accounts);
    Ok(())
}

/// Print the welcome banner explaining what rara-trading needs.
fn print_welcome() {
    eprintln!();
    eprintln!("  rara-trading — interactive setup");
    eprintln!("  ================================");
    eprintln!();
    eprintln!("  rara-trading needs two things to run:");
    eprintln!();
    eprintln!("    1. A PostgreSQL database (for market data & strategy results)");
    eprintln!("    2. An LLM agent CLI     (for AI-driven research)");
    eprintln!();
    eprintln!("  This wizard will check each one, help you configure it,");
    eprintln!("  and then set up your trading accounts.");
    eprintln!();
}

// ---------------------------------------------------------------------------
// Step 1 — Database
// ---------------------------------------------------------------------------

/// Available LLM backend choices for the select prompt.
const BACKEND_OPTIONS: &[&str] = &[
    "claude", "codex", "gemini", "kiro", "amp", "copilot", "opencode", "pi", "roo",
];

/// Step 1: detect database connectivity and configure if needed.
///
/// Loads (or creates) the config, tests the database URL, and lets the user
/// change it if the connection fails.
async fn step_database() -> error::Result<AppConfig> {
    eprintln!("[1/3] Database (PostgreSQL / TimescaleDB)");
    eprintln!("-----------------------------------------");

    let mut cfg = load_or_create_config()?;

    loop {
        eprintln!("  Testing connection: {}", cfg.database.url);
        eprintln!();

        match validation::validate_database(&cfg.database.url).await {
            Ok(()) => {
                eprintln!("  Connected successfully.");
                eprintln!();
                break;
            }
            Err(e) => {
                eprintln!("  Connection failed: {e}");
                eprintln!();
                eprintln!("  Make sure PostgreSQL is running and the URL is correct.");
                eprintln!("  Format: postgres://USER:PASS@HOST:PORT/DBNAME");
                eprintln!();

                if !confirm("  Enter a different database URL?", true)? {
                    eprintln!("  Keeping current URL. You can fix this later in config.toml.");
                    eprintln!();
                    break;
                }

                let new_url = input("  Database URL", Some(&cfg.database.url))?;
                cfg.database.url = new_url;
                app_config::save(&cfg).context(IoSnafu)?;
                eprintln!();
            }
        }
    }

    Ok(cfg)
}

// ---------------------------------------------------------------------------
// Step 2 — LLM Backend
// ---------------------------------------------------------------------------

/// Step 2: detect LLM backend availability and configure if needed.
///
/// Checks whether the selected backend CLI is in PATH. If not, lets the user
/// pick a different one or continue anyway.
fn step_llm_backend(mut cfg: AppConfig) -> error::Result<AppConfig> {
    eprintln!("[2/3] LLM Agent Backend");
    eprintln!("-----------------------");
    eprintln!("  rara-trading uses an LLM agent CLI to generate and evaluate");
    eprintln!("  trading strategies. It needs one of these tools in your PATH.");
    eprintln!();

    loop {
        let current = &cfg.agent.backend;
        eprintln!("  Current backend: {current}");

        if validation::validate_llm_backend(current).is_ok() {
            eprintln!("  Found `{current}` in PATH.");
            eprintln!();

            if !confirm("  Keep this backend?", true)? {
                cfg = pick_backend(cfg)?;
                continue;
            }

            break;
        }

        eprintln!("  `{current}` not found in PATH.");
        eprintln!();
        eprintln!("  You can:");
        eprintln!("    a) Install it and re-run this setup");
        eprintln!("    b) Pick a different backend that's already installed");
        eprintln!("    c) Skip for now and configure later in config.toml");
        eprintln!();

        if !confirm("  Pick a different backend?", true)? {
            eprintln!("  Keeping `{current}`. Install it before running `rara`.");
            eprintln!();
            break;
        }

        cfg = pick_backend(cfg)?;
    }

    Ok(cfg)
}

/// Present the backend selection menu and save the choice.
fn pick_backend(mut cfg: AppConfig) -> error::Result<AppConfig> {
    let current_idx = BACKEND_OPTIONS
        .iter()
        .position(|&b| b == cfg.agent.backend)
        .unwrap_or(0);
    let idx = select("  LLM backend", BACKEND_OPTIONS, current_idx)?;
    cfg.agent.backend = BACKEND_OPTIONS[idx].to_string();
    app_config::save(&cfg).context(IoSnafu)?;
    eprintln!();
    Ok(cfg)
}

// ---------------------------------------------------------------------------
// Step 3 — Trading accounts
// ---------------------------------------------------------------------------

/// Step 3: interactively add trading accounts.
///
/// Shows existing accounts and lets the user add new ones.
/// Returns the list of account IDs added during this session.
fn step_accounts() -> error::Result<Vec<String>> {
    eprintln!("[3/3] Trading Accounts");
    eprintln!("----------------------");

    let existing = accounts_config::load_accounts();
    let existing_count = existing.accounts.len();

    if existing_count > 0 {
        eprintln!("  You have {existing_count} account(s) configured:");
        for acc in &existing.accounts {
            let broker = match &acc.broker_config {
                BrokerConfig::Paper(_) => "paper",
                BrokerConfig::Ccxt(c) => &c.exchange,
            };
            let status = if acc.enabled { "enabled" } else { "disabled" };
            eprintln!("    - {} ({broker}, {status})", acc.id);
        }
    } else {
        eprintln!("  No accounts configured yet.");
        eprintln!("  You need at least one account to start trading.");
        eprintln!();
        eprintln!("  Account types:");
        eprintln!("    - paper: simulated fills, no real money — great for testing");
        eprintln!("    - ccxt:  live exchange (Binance, Bybit, OKX) via CCXT");
    }
    eprintln!();

    let mut added: Vec<String> = Vec::new();

    loop {
        let prompt = if added.is_empty() && existing_count == 0 {
            "  Add a trading account?"
        } else {
            "  Add another account?"
        };
        let default_yes = added.is_empty() && existing_count == 0;

        if !confirm(prompt, default_yes)? {
            break;
        }

        eprintln!();
        if let Some(id) = add_account_interactive()? {
            added.push(id);
        }
    }

    eprintln!();
    Ok(added)
}

/// Prompt the user interactively for account details and save to accounts.toml.
///
/// Returns `Some(id)` on success or `None` if the account already exists.
/// Sensitive fields (API key, secret, passphrase) use masked input.
fn add_account_interactive() -> error::Result<Option<String>> {
    let id = input(
        "  Account ID (unique identifier, e.g. binance-main, paper-test)",
        None,
    )?;

    let cfg = accounts_config::load_accounts();
    if cfg.accounts.iter().any(|a| a.id == id) {
        eprintln!("  Account \"{id}\" already exists — skipping.\n");
        return Ok(None);
    }

    let broker_options = &[
        "paper — simulated fills, no real money",
        "ccxt  — live exchange via CCXT",
    ];
    let broker_idx = select("  Broker type", broker_options, 0)?;
    let broker_type = if broker_idx == 0 { "paper" } else { "ccxt" };

    let label = input("  Label (display name)", Some(&id))?;

    let contracts_str = input(
        "  Contracts (comma-separated, e.g. BTC-USDT,ETH-USDT)",
        Some(""),
    )?;
    let contracts: Vec<String> = contracts_str
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    let broker_config = match broker_type {
        "paper" => build_paper_config()?,
        "ccxt" => build_ccxt_config()?,
        _ => unreachable!(),
    };

    let mut cfg = accounts_config::load_accounts();
    cfg.accounts.push(AccountConfig {
        id: id.clone(),
        label: Some(label),
        broker_config,
        enabled: true,
        contracts,
    });
    accounts_config::save_accounts(&cfg).context(IoSnafu)?;
    eprintln!("  Account \"{id}\" saved.\n");

    Ok(Some(id))
}

/// Build `BrokerConfig::Paper` from interactive prompts.
fn build_paper_config() -> error::Result<BrokerConfig> {
    eprintln!();
    eprintln!("  Paper broker — all orders are simulated locally.");
    let fill_price_str = input(
        "  Fixed fill price (leave empty to use market price)",
        Some(""),
    )?;
    let fill_price = fill_price_str.parse::<f64>().ok();
    Ok(BrokerConfig::Paper(PaperBrokerConfig { fill_price }))
}

/// Build `BrokerConfig::Ccxt` from interactive prompts.
///
/// Only asks for passphrase when OKX is selected.
fn build_ccxt_config() -> error::Result<BrokerConfig> {
    eprintln!();
    let exchange_options = &["binance", "bybit", "okx"];
    let exchange_idx = select("  Exchange", exchange_options, 0)?;
    let exchange = exchange_options[exchange_idx].to_string();

    let sandbox = confirm(
        "  Use sandbox/testnet? (recommended for first-time setup)",
        true,
    )?;

    eprintln!();
    eprintln!("  Enter your API credentials (input is hidden):");
    let api_key = password("  API key")?;
    let secret = password("  API secret")?;

    let passphrase = if exchange == "okx" {
        let p = password("  Passphrase (required for OKX)")?;
        if p.is_empty() { None } else { Some(p) }
    } else {
        None
    };

    Ok(BrokerConfig::Ccxt(CcxtBrokerConfig {
        exchange,
        sandbox,
        api_key,
        secret,
        passphrase,
    }))
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Load existing config from disk, or create a default one if none exists.
fn load_or_create_config() -> error::Result<AppConfig> {
    let config_path = paths::config_file();

    if config_path.exists() {
        let text = std::fs::read_to_string(&config_path).context(IoSnafu)?;
        Ok(toml::from_str(&text).unwrap_or_default())
    } else {
        let cfg = AppConfig::default();
        app_config::save(&cfg).context(IoSnafu)?;
        eprintln!("  Created {}", config_path.display());
        eprintln!();
        Ok(cfg)
    }
}

// ---------------------------------------------------------------------------
// Summary
// ---------------------------------------------------------------------------

/// Print a final summary of what was configured.
fn print_summary(cfg: &AppConfig, added_accounts: &[String]) {
    let accounts_cfg = accounts_config::load_accounts();
    let total = accounts_cfg.accounts.len();

    eprintln!("  ================================");
    eprintln!("  Setup complete!");
    eprintln!();
    eprintln!("  Database:    {}", cfg.database.url);
    eprintln!("  LLM backend: {}", cfg.agent.backend);
    eprintln!("  Accounts:    {total} configured");

    if !added_accounts.is_empty() {
        eprintln!("               ({} new: {})", added_accounts.len(), added_accounts.join(", "));
    }

    eprintln!();
    eprintln!("  Config: {}", paths::config_file().display());
    eprintln!();
    eprintln!("  Run `rara` to start trading. Happy trading!");
    eprintln!();
}
