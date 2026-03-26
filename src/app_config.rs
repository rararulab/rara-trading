//! Application configuration backed by TOML file.

use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use crate::agent::AgentConfig;

static APP_CONFIG: OnceLock<AppConfig> = OnceLock::new();

/// Application configuration.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    /// Agent backend configuration.
    pub agent: AgentConfig,
    /// Database configuration.
    pub database: DatabaseConfig,
    /// Trading engine configuration.
    pub trading: TradingConfig,
    /// Research loop configuration.
    pub research: ResearchConfig,
    /// Feedback evaluation configuration.
    pub feedback: FeedbackConfig,
    /// Sentinel monitoring configuration.
    pub sentinel: SentinelConfig,
    /// gRPC server configuration.
    pub server: ServerConfig,
}

/// Database connection configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DatabaseConfig {
    /// `PostgreSQL` connection URL.
    pub url: String,
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            url: "postgres://rara:rara@localhost:5432/rara_trading".to_string(),
        }
    }
}

/// Trading engine configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TradingConfig {
    /// Broker type (e.g. "paper", "binance").
    pub broker: String,
    /// Contracts to trade, e.g. `BTC-USDT`.
    pub contracts: Vec<String>,
    /// Maximum position size per contract.
    pub max_position_size: f64,
    /// Maximum total drawdown percentage before halting.
    pub max_drawdown_pct: f64,
    /// Maximum number of concurrent positions.
    pub max_concurrent_positions: u32,
}

impl Default for TradingConfig {
    fn default() -> Self {
        Self {
            broker: "paper".to_string(),
            contracts: vec!["BTC-USDT".to_string()],
            max_position_size: 1.0,
            max_drawdown_pct: 5.0,
            max_concurrent_positions: 3,
        }
    }
}

/// Research loop configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ResearchConfig {
    /// Number of iterations per research run.
    pub iterations: u32,
    /// Timeframes to backtest against.
    pub timeframes: Vec<String>,
    /// Maximum compile retries per hypothesis.
    pub max_compile_retries: u32,
}

impl Default for ResearchConfig {
    fn default() -> Self {
        Self {
            iterations: 5,
            timeframes: vec!["1h".to_string(), "4h".to_string(), "1d".to_string()],
            max_compile_retries: 3,
        }
    }
}

/// Feedback evaluation configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct FeedbackConfig {
    /// Minimum Sharpe ratio for promotion.
    pub min_sharpe_for_promotion: f64,
    /// Minimum win rate for promotion.
    pub min_win_rate: f64,
    /// Minimum trade count before evaluation.
    pub min_trades: u32,
    /// Maximum drawdown percentage for retirement.
    pub max_drawdown_for_retirement: f64,
}

impl Default for FeedbackConfig {
    fn default() -> Self {
        Self {
            min_sharpe_for_promotion: 1.0,
            min_win_rate: 0.45,
            min_trades: 30,
            max_drawdown_for_retirement: 20.0,
        }
    }
}

/// Sentinel monitoring configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SentinelConfig {
    /// Whether sentinel monitoring is enabled.
    pub enabled: bool,
    /// Interval in seconds between health checks.
    pub check_interval_secs: u64,
}

impl Default for SentinelConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            check_interval_secs: 30,
        }
    }
}

/// gRPC server configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    /// gRPC server listen address.
    pub listen_addr: String,
    /// gRPC server port.
    pub port: u16,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            listen_addr: "127.0.0.1".to_string(),
            port: 50051,
        }
    }
}

/// Load config from TOML file, falling back to defaults.
///
/// The result is cached in a `OnceLock` — subsequent calls return the same
/// value even after [`save`]. This is fine for CLI usage (one command per
/// process) but callers using this as a library should be aware of the
/// caching behavior.
pub fn load() -> &'static AppConfig {
    APP_CONFIG.get_or_init(|| {
        let path = crate::paths::config_file();
        if path.exists() {
            let settings = config::Config::builder()
                .add_source(config::File::from(path.as_ref()))
                .build()
                .unwrap_or_default();
            settings.try_deserialize().unwrap_or_default()
        } else {
            AppConfig::default()
        }
    })
}

/// Save config to TOML file.
pub fn save(cfg: &AppConfig) -> std::io::Result<()> {
    let path = crate::paths::config_file();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let content = toml::to_string_pretty(cfg).expect("config serialization should not fail");
    std::fs::write(path, content)
}

/// Generate a commented TOML config template with all sections and defaults.
pub fn generate_template() -> String {
    r#"# rara-trading configuration
# Generated by `rara config init`

# ─── Agent Backend ───────────────────────────────────────────────
[agent]
# LLM backend to use (e.g. "claude", "codex")
backend = "claude"
# Custom command override (optional)
# command = "my-custom-agent"
# Idle timeout in seconds (0 = no timeout)
idle_timeout_secs = 120

# ─── Database ────────────────────────────────────────────────────
[database]
# PostgreSQL connection URL (TimescaleDB)
url = "postgres://rara:rara@localhost:5432/rara_trading"

# ─── Trading Engine ──────────────────────────────────────────────
[trading]
# Broker type: "paper" for simulation, "binance" for live
broker = "paper"
# Contracts to trade
contracts = ["BTC-USDT"]
# Maximum position size per contract (in base units)
max_position_size = 1.0
# Maximum total drawdown percentage before halting trading
max_drawdown_pct = 5.0
# Maximum number of concurrent open positions
max_concurrent_positions = 3

# ─── Research Loop ───────────────────────────────────────────────
[research]
# Number of iterations per research run
iterations = 5
# Timeframes to backtest against
timeframes = ["1h", "4h", "1d"]
# Maximum compile retries per hypothesis before giving up
max_compile_retries = 3

# ─── Feedback Evaluation ─────────────────────────────────────────
[feedback]
# Minimum Sharpe ratio required to promote a strategy
min_sharpe_for_promotion = 1.0
# Minimum win rate required to promote a strategy
min_win_rate = 0.45
# Minimum number of trades before a strategy can be evaluated
min_trades = 30
# Maximum drawdown percentage before retiring a strategy
max_drawdown_for_retirement = 20.0

# ─── Sentinel Monitoring ─────────────────────────────────────────
[sentinel]
# Enable or disable sentinel health monitoring
enabled = false
# Interval in seconds between health checks
check_interval_secs = 30

# ─── gRPC Server ─────────────────────────────────────────────────
[server]
# Address the gRPC server listens on
listen_addr = "127.0.0.1"
# Port the gRPC server listens on
port = 50051
"#
    .to_string()
}
