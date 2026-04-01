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
        prompt:  String,
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

    /// Run the full trading loop: research, trading, feedback, and gRPC
    /// server as concurrent tasks in a single process.
    ///
    /// Accounts and contracts are loaded from `accounts.toml`.
    Run {
        /// Number of research iterations per cycle.
        #[arg(long, default_value = "10")]
        iterations: u32,
        /// gRPC server listen address.
        #[arg(long, default_value = "0.0.0.0:50051")]
        grpc_addr:  String,
    },

    /// Setup and configuration management.
    ///
    /// Use `-i` for interactive guided setup.
    Setup {
        /// Run interactive guided setup.
        #[arg(short = 'i', long = "interactive")]
        interactive: bool,

        #[command(subcommand)]
        action: Option<SetupAction>,
    },

    /// Start the gRPC server.
    Serve {
        /// Port to listen on.
        #[arg(long, default_value = "50051")]
        port: u16,
    },

    /// Launch the TUI dashboard.
    ///
    /// When `--server` is provided, connects to an existing gRPC server.
    /// When omitted, automatically spawns a server subprocess (standalone
    /// mode).
    Tui {
        /// gRPC server address to connect to. Omit for standalone mode.
        #[arg(long)]
        server: Option<String>,
    },

    /// Feedback loop operations.
    Feedback {
        #[command(subcommand)]
        action: FeedbackAction,
    },

    /// Live/sandbox trading operations.
    Trade {
        #[command(subcommand)]
        action: TradeAction,
    },

    /// Manage strategies from the rara-strategies registry.
    Strategy {
        #[command(subcommand)]
        action: StrategyAction,
    },

    /// Query events stored in the event bus.
    Events {
        #[command(subcommand)]
        action: EventsAction,
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
        limit:    usize,
    },
}

/// Trading subcommands.
#[derive(Subcommand, Debug)]
pub enum TradeAction {
    /// Start trading with promoted strategies.
    ///
    /// Accounts and contracts are loaded from `accounts.toml`.
    Start,

    /// Show trading status (strategies, positions, `PnL`).
    Status,

    /// Stop trading gracefully.
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
        start:  String,
        /// End date (YYYY-MM-DD).
        #[arg(long)]
        end:    String,
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
        contract:   String,
        /// Path to trace storage directory.
        #[arg(long)]
        trace_dir:  Option<String>,
        /// Only output final summary, suppress per-iteration progress.
        #[arg(long)]
        quiet:      bool,
    },

    /// List experiment history from the trace.
    List {
        /// Maximum number of entries to show.
        #[arg(long, default_value = "20")]
        limit:     usize,
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
        trace_dir:     Option<String>,
    },

    /// List promoted strategies.
    Promoted {
        /// Directory where promoted strategies are stored.
        #[arg(long)]
        promoted_dir: Option<String>,
    },
}

/// Strategy registry subcommands.
#[derive(Subcommand, Debug)]
pub enum StrategyAction {
    /// List available strategies from the rara-strategies GitHub registry.
    List {
        /// GitHub repository in "owner/repo" format.
        #[arg(long, default_value = "rararulab/rara-strategies")]
        repo: String,
    },

    /// Fetch a strategy from the registry, validate it, and install locally.
    Fetch {
        /// Strategy name to fetch (e.g. "btc-momentum").
        name: String,
        /// GitHub repository in "owner/repo" format.
        #[arg(long, default_value = "rararulab/rara-strategies")]
        repo: String,
    },

    /// List locally installed strategies fetched from the registry.
    Installed,

    /// Run a backtest on a fetched strategy against historical data.
    Backtest {
        /// Strategy name (e.g. "btc-momentum").
        name:      String,
        /// Contract/instrument to backtest against (e.g. "BTCUSDT").
        #[arg(long)]
        contract:  String,
        /// Timeframe for the backtest (e.g. "1h", "4h", "1d").
        #[arg(long, default_value = "1h")]
        timeframe: String,
    },
}

/// Event bus query subcommands.
#[derive(Subcommand, Debug)]
pub enum EventsAction {
    /// List all events that share a given correlation ID.
    ///
    /// Performs a full scan of the event store. Intended for debugging and
    /// pipeline tracing, not high-frequency use.
    Query {
        /// Correlation ID to filter by.
        #[arg(long)]
        correlation_id: String,
        /// Maximum number of events to show.
        #[arg(long, default_value = "50")]
        limit:          usize,
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

/// Setup subcommands.
#[derive(Subcommand, Debug)]
pub enum SetupAction {
    /// Generate config.toml and accounts.toml templates.
    Init {
        /// Overwrite existing files.
        #[arg(long)]
        force: bool,
    },
    /// Manage trading accounts.
    Account {
        #[command(subcommand)]
        action: Box<SetupAccountAction>,
    },
    /// Validate all configuration files.
    Validate,
    /// Download historical market data for backtesting.
    ///
    /// Without arguments, downloads BTC + ETH (10 years, 1m candles).
    /// Use `--search` to find and add other symbols from Binance.
    Data {
        /// Data source: "binance" or "yahoo".
        #[arg(long, default_value = "binance")]
        source:  String,
        /// Search for a symbol (e.g. "SOL", "DOGE").
        #[arg(long)]
        search:  Option<String>,
        /// Start date (YYYY-MM-DD). Defaults to earliest available on exchange.
        #[arg(long)]
        start:   Option<String>,
        /// End date (YYYY-MM-DD). Defaults to today.
        #[arg(long)]
        end:     Option<String>,
        /// Symbols to download (e.g. BTCUSDT ETHUSDT). Defaults to BTC + ETH.
        symbols: Vec<String>,
    },
}

/// Account management subcommands.
#[derive(Subcommand, Debug)]
pub enum SetupAccountAction {
    /// Add a trading account.
    ///
    /// EXAMPLES:
    ///     rara setup account add --id binance-sandbox --exchange binance
    /// --sandbox --api-key "$KEY" --secret "$SECRET"     rara setup account
    /// add --id binance-prod --exchange binance --api-key "$KEY" --secret
    /// "$SECRET"
    Add {
        /// Account identifier.
        #[arg(long)]
        id:         String,
        /// Human-readable label.
        #[arg(long)]
        label:      Option<String>,
        /// Contracts to trade (comma-separated).
        #[arg(long, value_delimiter = ',')]
        contracts:  Option<Vec<String>>,
        /// Enable/disable the account.
        #[arg(long, default_value = "true")]
        enabled:    bool,
        /// Exchange: "binance", "bybit", "okx".
        #[arg(long)]
        exchange:   String,
        /// API key.
        #[arg(long)]
        api_key:    Option<String>,
        /// API secret.
        #[arg(long)]
        secret:     Option<String>,
        /// API passphrase (OKX only).
        #[arg(long)]
        passphrase: Option<String>,
        /// Use exchange sandbox/testnet environment.
        #[arg(long)]
        sandbox:    bool,
    },
    /// List configured accounts.
    List,
    /// Remove a trading account.
    ///
    /// EXAMPLES:
    ///     rara setup account remove binance-prod --yes
    Remove {
        /// Account ID to remove.
        id:  String,
        /// Skip confirmation.
        #[arg(long)]
        yes: bool,
    },
    /// Test broker connectivity for an account.
    ///
    /// EXAMPLES:
    ///     rara setup account test binance-prod
    Test {
        /// Account ID to test.
        id: String,
    },
}
