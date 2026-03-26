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

    /// Fetch and store historical market data.
    Data {
        #[command(subcommand)]
        action: DataAction,
    },

    /// Run the full trading loop: research, paper trading, feedback, and gRPC
    /// server as concurrent tasks in a single process.
    Run {
        /// Contracts to trade (comma-separated).
        #[arg(long, default_value = "BTC-USDT")]
        contracts: String,
        /// Number of research iterations per cycle.
        #[arg(long, default_value = "10")]
        iterations: u32,
        /// gRPC server listen address.
        #[arg(long, default_value = "0.0.0.0:50051")]
        grpc_addr: String,
    },

    /// Validate configuration and check connectivity.
    Validate,

    /// Start the gRPC server.
    Serve {
        /// Port to listen on.
        #[arg(long, default_value = "50051")]
        port: u16,
    },

    /// Launch the TUI dashboard.
    Tui {
        /// gRPC server address to connect to.
        #[arg(long, default_value = "http://127.0.0.1:50051")]
        server: String,
    },

    /// Feedback loop operations.
    Feedback {
        #[command(subcommand)]
        action: FeedbackAction,
    },


    /// Paper trading operations.
    Paper {
        #[command(subcommand)]
        action: PaperAction,
    },
}

/// Feedback loop subcommands.
#[derive(Subcommand, Debug)]
pub enum FeedbackAction {
    /// Show strategy evaluation history.
    Report {
        /// Filter by strategy ID.
        #[arg(long)]
        strategy: Option<String>,
        /// Maximum number of entries to show.
        #[arg(long, default_value = "20")]
        limit: usize,
    },
}


/// Paper trading subcommands.
#[derive(Subcommand, Debug)]
pub enum PaperAction {
    /// Start paper trading with promoted strategies.
    Start {
        /// Override contracts to trade (comma-separated, e.g. "BTCUSDT,ETHUSDT").
        #[arg(long)]
        contracts: Option<String>,
    },

    /// Show paper trading status (strategies, positions, `PnL`).
    Status,

    /// Stop paper trading gracefully.
    Stop,
}

/// Data management subcommands.
#[derive(Subcommand, Debug)]
pub enum DataAction {
    /// Fetch historical candles from an exchange into `TimescaleDB`.
    /// Days already fully stored are skipped automatically.
    Fetch {
        /// Data source: "binance" or "yahoo".
        #[arg(long)]
        source: String,
        /// Instrument symbol, e.g. "BTCUSDT" (Binance) or "SPY" (Yahoo).
        #[arg(long)]
        symbol: String,
        /// Start date (YYYY-MM-DD).
        #[arg(long)]
        start: String,
        /// End date (YYYY-MM-DD).
        #[arg(long)]
        end: String,
    },

    /// Show data coverage for all stored instruments (JSON output).
    Info,
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
        /// Only output final summary, suppress per-iteration progress.
        #[arg(long)]
        quiet: bool,
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

/// Config management subcommands.
#[derive(Subcommand)]
pub enum ConfigAction {
    /// Generate a config.toml template with all sections and comments
    Init {
        /// Overwrite existing config file if present
        #[arg(long)]
        force: bool,
    },
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
