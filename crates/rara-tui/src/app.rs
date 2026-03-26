//! Application state for the TUI dashboard.

use rara_server::rara_proto::SystemStatus;

/// Live status of a running strategy.
#[derive(Debug, Clone)]
pub struct StrategyStatus {
    /// Human-readable strategy name.
    pub name: String,
    /// Current lifecycle state (e.g. "Running", "Promoted", "Paper", "Stopped").
    pub status: String,
    /// Cumulative profit-and-loss.
    pub pnl: f64,
    /// Annualized Sharpe ratio.
    pub sharpe: f64,
}

/// Snapshot of an open position.
#[derive(Debug, Clone)]
pub struct PositionInfo {
    /// Trading pair symbol (e.g. "BTCUSDT").
    pub symbol: String,
    /// Position direction: "Long" or "Short".
    pub side: String,
    /// Position size.
    pub quantity: f64,
    /// Average entry price.
    pub entry_price: f64,
    /// Latest market price.
    pub current_price: f64,
    /// Unrealized profit-and-loss.
    pub pnl: f64,
}

/// A single event for the recent-events feed.
#[derive(Debug, Clone)]
pub struct RecentEvent {
    /// Formatted timestamp (e.g. "14:32:01").
    pub time: String,
    /// Event category tag (e.g. "TRADE", "ERROR", "INFO").
    pub event_type: String,
    /// One-line event description.
    pub summary: String,
}

/// Aggregate progress of the autonomous research pipeline.
#[derive(Debug, Clone)]
pub struct ResearchProgress {
    /// Number of backtests completed so far.
    pub current: u32,
    /// Total backtests planned for this sweep.
    pub total: u32,
    /// Strategies that passed acceptance criteria.
    pub accepted: u32,
    /// Strategies that failed acceptance criteria.
    pub rejected: u32,
    /// Strategies currently being evaluated.
    pub in_progress: u32,
    /// Best Sharpe ratio discovered so far, if any.
    pub sota_sharpe: Option<f64>,
}

/// Connection state between the TUI client and the gRPC server.
#[derive(Debug, Clone)]
pub enum ConnectionStatus {
    /// Currently attempting to connect.
    Connecting,
    /// Successfully connected and receiving data.
    Connected,
    /// Disconnected from the server with retry tracking.
    Disconnected {
        /// Number of reconnection attempts made so far.
        retry_count: u32,
    },
}

/// Tab identifiers for the main navigation.
pub const TAB_NAMES: &[&str] = &["Overview", "Research", "Trading", "Strategies"];

/// Root application state driving the TUI.
pub struct App {
    /// Index of the currently active tab.
    pub active_tab: usize,
    /// Whether the application is still running.
    pub running: bool,
    /// Current connection state to the gRPC server.
    pub connection_status: ConnectionStatus,
    /// Last received system status from the server.
    pub system_status: Option<SystemStatus>,
    /// gRPC server address the client connects to.
    pub server_addr: String,
    /// Live strategy statuses displayed on the overview tab.
    pub strategies: Vec<StrategyStatus>,
    /// Open positions displayed on the overview tab.
    pub positions: Vec<PositionInfo>,
    /// Rolling window of recent events.
    pub recent_events: Vec<RecentEvent>,
    /// Active alert messages.
    pub alerts: Vec<String>,
    /// Current research pipeline progress, if any.
    pub research_progress: Option<ResearchProgress>,
}

impl App {
    /// Create a new app instance targeting the given gRPC server address.
    #[must_use]
    pub const fn new(server_addr: String) -> Self {
        Self {
            active_tab: 0,
            running: true,
            connection_status: ConnectionStatus::Connecting,
            system_status: None,
            server_addr,
            strategies: Vec::new(),
            positions: Vec::new(),
            recent_events: Vec::new(),
            alerts: Vec::new(),
            research_progress: None,
        }
    }

    /// Switch to a tab by index (0-based). Out-of-range values are ignored.
    pub const fn select_tab(&mut self, index: usize) {
        if index < TAB_NAMES.len() {
            self.active_tab = index;
        }
    }

    /// Signal the application to quit.
    pub const fn quit(&mut self) {
        self.running = false;
    }
}
