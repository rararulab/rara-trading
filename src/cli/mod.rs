//! CLI command definitions and subcommand modules.

use clap::{Parser, Subcommand};

/// Your CLI application — update this doc comment.
#[derive(Parser)]
#[command(name = "rara-trading", version)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

/// Available subcommands.
#[derive(Subcommand)]
pub enum Command {
    /// Say hello (example command — replace with your own)
    Hello {
        /// Name to greet
        #[arg(default_value = "world")]
        name: String,
    },

    /// Manage config values
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },

    /// Run a prompt through the configured agent backend
    Agent {
        /// The prompt to send to the agent
        prompt: String,
        /// Override the backend (e.g., "claude", "codex")
        #[arg(long)]
        backend: Option<String>,
    },

    /// Run the research loop to iterate on trading strategies.
    Research {
        #[command(subcommand)]
        action: ResearchAction,
    },

    /// Fetch and manage market data.
    Data {
        #[command(subcommand)]
        action: DataAction,
    },
}

/// Research loop subcommands.
#[derive(Subcommand, Debug)]
pub enum ResearchAction {
    /// Run N iterations of the research loop.
    Run {
        /// Number of iterations to run.
        #[arg(long, default_value = "5")]
        iterations: u32,
        /// Contract to backtest against.
        #[arg(long, default_value = "BTC-USDT")]
        contract: String,
        /// Path to trace storage directory.
        #[arg(long)]
        trace_dir: Option<String>,
    },

    /// List experiment history from the trace.
    List {
        /// Maximum number of entries to show.
        #[arg(long, default_value = "20")]
        limit: usize,
        /// Path to trace storage directory.
        #[arg(long)]
        trace_dir: Option<String>,
    },

    /// Show full details of a specific experiment.
    Show {
        /// Experiment ID to display.
        #[arg(long)]
        experiment_id: String,
        /// Path to trace storage directory.
        #[arg(long)]
        trace_dir: Option<String>,
    },

    /// List promoted strategies.
    Promoted {
        /// Directory where promoted strategies are stored.
        #[arg(long)]
        promoted_dir: Option<String>,
    },
}

/// Data management subcommands.
#[derive(Subcommand)]
pub enum DataAction {
    /// Fetch historical candle data from an exchange.
    Fetch {
        /// Data source: "binance" or "yahoo".
        #[arg(long)]
        source: String,
        /// Symbol to fetch, e.g. "BTCUSDT" for Binance or "SPY" for Yahoo.
        #[arg(long)]
        symbol: String,
        /// Start date (YYYY-MM-DD).
        #[arg(long)]
        start: String,
        /// End date (YYYY-MM-DD), defaults to today.
        #[arg(long)]
        end: Option<String>,
    },
}

/// Config management subcommands.
#[derive(Subcommand)]
pub enum ConfigAction {
    /// Set a config value
    Set {
        /// Config key (e.g. example.setting)
        key:   String,
        /// Config value
        value: String,
    },
    /// Get a config value
    Get {
        /// Config key to look up
        key: String,
    },
    /// List all config values
    List,
}
