//! Interactive setup wizard — guides the user through config init, account
//! creation, and validation via terminal prompts.

use dialoguer::{Confirm, Input, Password, Select};
use snafu::ResultExt;

use crate::accounts_config;
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

/// Interactive guided setup — walks the user through init, account creation, and validation.
///
/// Requires a TTY on stdin; returns an error if prompts cannot be displayed.
pub async fn run(
    init_fn: fn(bool) -> error::Result<()>,
    validate_fn: impl AsyncFn() -> error::Result<()>,
) -> error::Result<()> {
    eprintln!("=== rara-trading interactive setup ===\n");

    // --- Step 1: Init config files ---
    let config_path = paths::config_file();
    let accounts_path = paths::accounts_file();
    let config_exists = config_path.exists();
    let accounts_exists = accounts_path.exists();

    if config_exists && accounts_exists {
        eprintln!("Config files already exist:");
        eprintln!("  {}", config_path.display());
        eprintln!("  {}", accounts_path.display());
        let overwrite = Confirm::new()
            .with_prompt("Overwrite existing config files?")
            .default(false)
            .interact()
            .map_err(dialog_io)
            .context(IoSnafu)?;
        if overwrite {
            init_fn(true)?;
        }
    } else {
        eprintln!("Generating config files...");
        init_fn(false)?;
    }

    // --- Step 2: Add accounts ---
    loop {
        let add_account = Confirm::new()
            .with_prompt("Add a trading account?")
            .default(true)
            .interact()
            .map_err(dialog_io)
            .context(IoSnafu)?;

        if !add_account {
            break;
        }

        add_account_interactive()?;
    }

    // --- Step 3: Validate ---
    let validate = Confirm::new()
        .with_prompt("Run validation checks?")
        .default(true)
        .interact()
        .map_err(dialog_io)
        .context(IoSnafu)?;

    if validate {
        eprintln!("\nRunning validation...");
        validate_fn().await?;
    }

    eprintln!("\n=== Setup complete ===");
    Ok(())
}

/// Prompt the user interactively for account details and save to accounts.toml.
///
/// Sensitive fields (API key, secret, passphrase) use masked input.
#[allow(clippy::result_large_err, clippy::too_many_lines)]
fn add_account_interactive() -> error::Result<()> {
    let id: String = Input::new()
        .with_prompt("Account ID")
        .interact_text()
        .map_err(dialog_io)
        .context(IoSnafu)?;

    let broker_options = &["paper", "ccxt"];
    let broker_idx = Select::new()
        .with_prompt("Broker type")
        .items(broker_options)
        .default(0)
        .interact()
        .map_err(dialog_io)
        .context(IoSnafu)?;
    let broker_type = broker_options[broker_idx];

    let label: String = Input::new()
        .with_prompt("Label (human-readable name)")
        .default(id.clone())
        .interact_text()
        .map_err(dialog_io)
        .context(IoSnafu)?;

    let contracts_str: String = Input::new()
        .with_prompt("Contracts (comma-separated, e.g. BTC-USDT,ETH-USDT)")
        .default(String::new())
        .interact_text()
        .map_err(dialog_io)
        .context(IoSnafu)?;

    let contracts: Vec<String> = contracts_str
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    let broker_config = match broker_type {
        "paper" => {
            let fill_price_str: String = Input::new()
                .with_prompt("Fill price (leave empty for market price)")
                .default(String::new())
                .interact_text()
                .map_err(dialog_io)
                .context(IoSnafu)?;
            let fill_price = fill_price_str.parse::<f64>().ok();
            BrokerConfig::Paper(PaperBrokerConfig { fill_price })
        }
        "ccxt" => {
            let exchange_options = &["binance", "bybit", "okx"];
            let exchange_idx = Select::new()
                .with_prompt("Exchange")
                .items(exchange_options)
                .default(0)
                .interact()
                .map_err(dialog_io)
                .context(IoSnafu)?;
            let exchange = exchange_options[exchange_idx].to_string();

            let sandbox = Confirm::new()
                .with_prompt("Use sandbox/testnet?")
                .default(true)
                .interact()
                .map_err(dialog_io)
                .context(IoSnafu)?;

            let api_key = Password::new()
                .with_prompt("API key (input hidden)")
                .allow_empty_password(true)
                .interact()
                .map_err(dialog_io)
                .context(IoSnafu)?;

            let secret = Password::new()
                .with_prompt("API secret (input hidden)")
                .allow_empty_password(true)
                .interact()
                .map_err(dialog_io)
                .context(IoSnafu)?;

            let passphrase = Password::new()
                .with_prompt("Passphrase — OKX only, press Enter to skip (input hidden)")
                .allow_empty_password(true)
                .interact()
                .map_err(dialog_io)
                .context(IoSnafu)?;

            BrokerConfig::Ccxt(CcxtBrokerConfig {
                exchange,
                sandbox,
                api_key,
                secret,
                passphrase: if passphrase.is_empty() {
                    None
                } else {
                    Some(passphrase)
                },
            })
        }
        _ => unreachable!(),
    };

    let mut cfg = accounts_config::load_accounts();

    if cfg.accounts.iter().any(|a| a.id == id) {
        eprintln!("Account \"{id}\" already exists, skipping.");
        return Ok(());
    }

    cfg.accounts.push(AccountConfig {
        id: id.clone(),
        label: Some(label),
        broker_config,
        enabled: true,
        contracts,
    });

    accounts_config::save_accounts(&cfg).context(IoSnafu)?;
    eprintln!("Account \"{id}\" added successfully.\n");

    Ok(())
}
