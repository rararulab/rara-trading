//! Interactive setup wizard — detects prerequisites, guides configuration,
//! and validates the result so first-time users can get running painlessly.

use std::collections::HashMap;

use dialoguer::{Confirm, Input, Password, Select};
use snafu::ResultExt;

use crate::accounts_config;
use crate::app_config::{self, AppConfig};
use crate::error::{self, IoSnafu};
use crate::paths;
use crate::validation;

use rara_trading_engine::account_config::AccountConfig;
use rara_trading_engine::broker_registry::{
    BrokerRegistryEntry, ConfigField, ConfigFieldType, BROKER_REGISTRY,
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
    let accounts = step_accounts().await?;

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
async fn step_accounts() -> error::Result<Vec<String>> {
    eprintln!("[3/3] Trading Accounts");
    eprintln!("----------------------");

    let existing = accounts_config::load_accounts();
    let existing_count = existing.accounts.len();

    if existing_count > 0 {
        eprintln!("  You have {existing_count} account(s) configured:");
        for acc in &existing.accounts {
            let status = if acc.enabled { "enabled" } else { "disabled" };
            eprintln!("    - {} ({}, {status})", acc.id, acc.broker_config.type_key());
        }
    } else {
        eprintln!("  No accounts configured yet.");
        eprintln!("  You need at least one account to start trading.");
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
        if let Some(id) = add_account_interactive().await? {
            added.push(id);
        }
    }

    eprintln!();
    Ok(added)
}

/// Test broker connectivity by creating a broker instance and calling `account_info`.
async fn test_broker_connection(
    entry: &BrokerRegistryEntry,
    fields: &HashMap<String, String>,
) -> std::result::Result<rara_trading_engine::broker::AccountInfo, String> {
    let broker = (entry.create_broker)(fields).map_err(|e| e.to_string())?;
    broker.account_info().await.map_err(|e| e.to_string())
}

/// Prompt the user interactively for account details and save to accounts.toml.
///
/// Uses a broker-first flow: select broker type, collect broker-specific config,
/// then suggest a short account ID derived from broker context (e.g. `bybit`
/// for CCXT/Bybit, `paper` for Paper). Tests the connection before saving.
async fn add_account_interactive() -> error::Result<Option<String>> {
    // 1. Broker type selection
    let registry = &*BROKER_REGISTRY;
    let broker_labels: Vec<String> = registry
        .iter()
        .map(|e| format!("{} — {}", e.name, e.description))
        .collect();
    let broker_refs: Vec<&str> = broker_labels.iter().map(String::as_str).collect();
    let broker_idx = select("  Broker type", &broker_refs, 0)?;
    let entry = &registry[broker_idx];

    // 2. Collect broker-specific fields first (e.g. exchange for CCXT)
    eprintln!();
    eprintln!("  {} configuration:", entry.name);
    let fields = collect_config_fields(&(entry.config_fields)())?;

    // 3. Pick a unique account ID — suggest from broker context, retry on conflict
    let suggested_id = suggest_account_id(entry.type_key, &fields);
    let id = loop {
        let candidate = input("  Account ID", Some(&suggested_id))?;
        let cfg = accounts_config::load_accounts();
        if !cfg.accounts.iter().any(|a| a.id == candidate) {
            break candidate;
        }
        eprintln!(
            "  \"{candidate}\" already exists — try a different name \
             (e.g. {suggested_id}-demo, {suggested_id}-test)."
        );
    };

    // 4. Label defaults to broker name
    let label = input("  Label (display name)", Some(entry.name))?;

    // 5. Contracts
    let contracts_str = input(
        "  Contracts (comma-separated, e.g. BTC-USDT,ETH-USDT)",
        Some(""),
    )?;
    let contracts: Vec<String> = contracts_str
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    // 6. Test connection
    eprintln!();
    eprintln!("  Testing connection...");
    let broker_config = (entry.create_config)(&fields)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e.to_string()))
        .context(IoSnafu)?;

    let save = match test_broker_connection(entry, &fields).await {
        Ok(info) => {
            eprintln!(
                "  Connected! Equity: {}, Cash: {}",
                info.total_equity, info.available_cash
            );
            eprintln!();
            true
        }
        Err(e) => {
            eprintln!("  Connection failed: {e}");
            eprintln!();
            confirm("  Save account anyway?", false)?
        }
    };

    if !save {
        eprintln!("  Account not saved.\n");
        return Ok(None);
    }

    // 7. Save
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

/// Suggest an account ID based on broker type and collected config fields.
///
/// For CCXT brokers, uses the exchange name (e.g. `bybit`).
/// For other brokers, uses the type key (e.g. `paper`).
/// Returns a simple, descriptive base — no auto-incrementing suffixes.
fn suggest_account_id(type_key: &str, fields: &HashMap<String, String>) -> String {
    fields
        .get("exchange")
        .filter(|v| !v.is_empty())
        .cloned()
        .unwrap_or_else(|| type_key.to_string())
}

/// Dynamically collect config field values using dialoguer prompts.
fn collect_config_fields(fields: &[ConfigField]) -> error::Result<HashMap<String, String>> {
    let mut values = HashMap::new();

    for field in fields {
        if let Some(ref desc) = field.description {
            eprintln!("  {desc}");
        }

        let value = match field.field_type {
            ConfigFieldType::Password => password(&format!("  {}", field.label))?,
            ConfigFieldType::Select => {
                let labels: Vec<&str> = field.options.iter().map(|o| o.label.as_str()).collect();
                let default_idx = field
                    .default
                    .as_ref()
                    .and_then(|d| field.options.iter().position(|o| o.value == *d))
                    .unwrap_or(0);
                let idx = select(&format!("  {}", field.label), &labels, default_idx)?;
                field.options[idx].value.clone()
            }
            ConfigFieldType::Boolean => {
                let default = field.default.as_ref().is_some_and(|d| d == "true");
                let yes = confirm(&format!("  {}", field.label), default)?;
                yes.to_string()
            }
            ConfigFieldType::Text | ConfigFieldType::Number => input(
                &format!("  {}", field.label),
                field.default.as_deref().or(Some("")),
            )?,
        };

        values.insert(field.name.clone(), value);
    }

    Ok(values)
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
