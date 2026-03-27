//! Interactive setup wizard — guides the user through config init, account
//! creation, and validation via terminal prompts.

use dialoguer::{Confirm, Input, Password, Select};
use snafu::ResultExt;

use crate::accounts_config;
use crate::app_config::{self, AppConfig};
use crate::error::{self, IoSnafu};
use crate::paths;

use rara_trading_engine::account_config::{
    AccountConfig, BrokerConfig, CcxtBrokerConfig, PaperBrokerConfig,
};

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

/// Interactive guided setup — walks the user through config, accounts, and validation.
///
/// Displays a welcome banner, step indicators, contextual hints, and a final
/// summary so first-time users always know where they are and what to do next.
///
/// Requires a TTY on stdin; returns an error if prompts cannot be displayed.
pub async fn run(
    validate_fn: impl AsyncFn() -> error::Result<()>,
) -> error::Result<()> {
    print_welcome();

    step_configure()?;
    let accounts = step_add_accounts()?;
    step_validate(validate_fn).await?;

    print_summary(&accounts);
    Ok(())
}

/// Print the welcome banner and an overview of the three setup steps.
fn print_welcome() {
    eprintln!();
    eprintln!("  rara-trading — interactive setup");
    eprintln!("  ================================");
    eprintln!();
    eprintln!("  This wizard will walk you through three steps:");
    eprintln!();
    eprintln!("    1. Configure core settings (database, LLM backend)");
    eprintln!("    2. Add trading accounts");
    eprintln!("    3. Validate your setup");
    eprintln!();
}

// ---------------------------------------------------------------------------
// Step 1 — Core configuration
// ---------------------------------------------------------------------------

/// Available LLM backend choices for the select prompt.
const BACKEND_OPTIONS: &[&str] = &[
    "claude", "codex", "gemini", "kiro", "amp", "copilot", "opencode", "pi", "roo",
];

/// Step 1: interactively configure core settings and write `config.toml`.
///
/// Asks the user for the two most important values (database URL and LLM
/// backend) that have no sensible auto-detection, while keeping all other
/// settings at their defaults.  Existing values from a previous config file
/// are shown as defaults so returning users can just press Enter.
fn step_configure() -> error::Result<()> {
    eprintln!("[1/3] Configure core settings");
    eprintln!("-----------------------------");

    // Load existing config (or defaults) so we can pre-fill prompts
    let config_path = paths::config_file();
    let existing: AppConfig = if config_path.exists() {
        let text = std::fs::read_to_string(&config_path)
            .unwrap_or_default();
        toml::from_str(&text).unwrap_or_default()
    } else {
        AppConfig::default()
    };

    // -- Database URL --
    eprintln!();
    eprintln!("  PostgreSQL / TimescaleDB connection URL.");
    eprintln!("  This is where rara-trading stores market data and strategy results.");
    let db_url = input("  Database URL", Some(&existing.database.url))?;

    // -- LLM Backend --
    eprintln!();
    eprintln!("  Which LLM agent CLI should rara-trading use for research?");
    let current_backend_idx = BACKEND_OPTIONS
        .iter()
        .position(|&b| b == existing.agent.backend)
        .unwrap_or(0);
    let backend_idx = select("  LLM backend", BACKEND_OPTIONS, current_backend_idx)?;
    let backend = BACKEND_OPTIONS[backend_idx].to_string();

    // -- Build and save config --
    let mut cfg = existing;
    cfg.database.url = db_url;
    cfg.agent.backend = backend;
    app_config::save(&cfg).context(IoSnafu)?;

    // Also ensure accounts.toml exists
    let accounts_path = paths::accounts_file();
    if !accounts_path.exists() {
        let template = accounts_config::generate_accounts_template();
        std::fs::write(&accounts_path, &template).context(IoSnafu)?;
    }

    eprintln!();
    eprintln!("  Saved to {}", config_path.display());
    eprintln!();

    Ok(())
}

// ---------------------------------------------------------------------------
// Step 2 — Account creation
// ---------------------------------------------------------------------------

/// Step 2: interactively add one or more trading accounts.
///
/// Returns the list of account IDs that were added during this session.
fn step_add_accounts() -> error::Result<Vec<String>> {
    eprintln!("[2/3] Add trading accounts");
    eprintln!("--------------------------");
    eprintln!("  Each account connects to a broker:");
    eprintln!("    - paper: simulated fills, no real money — great for testing");
    eprintln!("    - ccxt:  live exchange (Binance, Bybit, OKX) via CCXT");
    eprintln!();

    let mut added: Vec<String> = Vec::new();

    loop {
        let prompt = if added.is_empty() {
            "  Add a trading account?"
        } else {
            "  Add another account?"
        };

        if !confirm(prompt, added.is_empty())? {
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

    // bail early on duplicate
    let cfg = accounts_config::load_accounts();
    if cfg.accounts.iter().any(|a| a.id == id) {
        eprintln!("  Account \"{id}\" already exists — skipping.\n");
        return Ok(None);
    }

    let broker_options = &["paper — simulated fills, no real money", "ccxt  — live exchange via CCXT"];
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

    // -- broker-specific config --
    let broker_config = match broker_type {
        "paper" => build_paper_config()?,
        "ccxt" => build_ccxt_config()?,
        _ => unreachable!(),
    };

    // -- save --
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

    // Only OKX requires a passphrase
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
// Step 3 — Validation
// ---------------------------------------------------------------------------

/// Step 3: optionally run validation checks (database, LLM backend).
async fn step_validate(validate_fn: impl AsyncFn() -> error::Result<()>) -> error::Result<()> {
    eprintln!("[3/3] Validate setup");
    eprintln!("--------------------");
    eprintln!("  Checks database connectivity and LLM backend availability.");
    eprintln!();

    if !confirm("  Run validation now?", true)? {
        eprintln!("  Skipped. You can run validation later with: rara setup validate\n");
        return Ok(());
    }

    eprintln!();
    match validate_fn().await {
        Ok(()) => {
            eprintln!("  All checks passed.\n");
        }
        Err(e) => {
            eprintln!("  Validation failed: {e}");
            eprintln!();
            eprintln!("  Troubleshooting tips:");
            eprintln!("    - Database: check RARA_DB_URL or [database] in config.toml");
            eprintln!("    - LLM backend: ensure the binary is in your PATH");
            eprintln!("    - Run `rara setup validate` to retry after fixing.");
            eprintln!();
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Summary
// ---------------------------------------------------------------------------

/// Print a final summary of what was accomplished during setup.
fn print_summary(added_accounts: &[String]) {
    eprintln!("  ================================");
    eprintln!("  Setup complete!");
    eprintln!();

    if added_accounts.is_empty() {
        eprintln!("  No new accounts were added.");
    } else {
        eprintln!(
            "  {} account(s) added: {}",
            added_accounts.len(),
            added_accounts.join(", ")
        );
    }

    let config_dir = paths::data_dir();
    eprintln!();
    eprintln!("  Config directory: {}", config_dir.display());
    eprintln!();
    eprintln!("  Next steps:");
    eprintln!("    - Edit config.toml to tune trading/research parameters");
    eprintln!("    - Run `rara setup validate` to re-check your setup");
    eprintln!("    - Run `rara` to start trading");
    eprintln!();
}
