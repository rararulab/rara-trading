//! Application state for the TUI dashboard.

use rara_server::rara_proto::SystemStatus;
use strum::{Display, EnumString};

use crate::tabs::research::ResearchState;

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
pub const TAB_NAMES: &[&str] = &["Overview", "Research", "Trading", "Strategies", "Events"];

/// Index of the Events tab in `TAB_NAMES`.
pub const EVENTS_TAB_INDEX: usize = 4;

/// A single event entry in the events stream.
#[derive(Debug, Clone)]
pub struct EventEntry {
    /// Monotonically increasing sequence number.
    pub seq: u64,
    /// Human-readable timestamp string.
    pub time: String,
    /// Event topic (trading, research, feedback, sentinel).
    pub topic: String,
    /// The type/kind of event within the topic.
    pub event_type: String,
    /// One-line summary of the event.
    pub summary: String,
    /// Optional strategy identifier associated with this event.
    pub strategy_id: Option<String>,
    /// Full event payload, typically JSON.
    pub payload: String,
}

/// Topic-level filter for the events stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventFilter {
    /// Show all events regardless of topic.
    All,
    /// Show only trading events.
    Trading,
    /// Show only research events.
    Research,
    /// Show only feedback events.
    Feedback,
    /// Show only sentinel events.
    Sentinel,
}

/// State for the Events tab.
pub struct EventsState {
    /// All received events (unfiltered buffer).
    pub events: Vec<EventEntry>,
    /// Current topic filter.
    pub filter: EventFilter,
    /// Whether auto-scroll is enabled (newest events stay visible).
    pub auto_scroll: bool,
    /// Currently selected row index within the filtered view.
    pub selected_index: usize,
    /// Current search query text.
    pub search_query: String,
    /// Whether the search input is actively accepting keystrokes.
    pub search_active: bool,
    /// Whether the detail pane is expanded.
    pub detail_expanded: bool,
}

impl Default for EventsState {
    fn default() -> Self {
        Self {
            events: Vec::new(),
            filter: EventFilter::All,
            auto_scroll: true,
            selected_index: 0,
            search_query: String::new(),
            search_active: false,
            detail_expanded: false,
        }
    }
}

/// Index of the Research tab.
pub const TAB_RESEARCH: usize = 1;

/// Index of the Trading tab.
pub const TRADING_TAB: usize = 2;

/// Account-level summary displayed in the trading tab header.
#[derive(Debug, Clone)]
pub struct AccountState {
    /// Total portfolio equity.
    pub equity: f64,
    /// Available cash balance.
    pub cash: f64,
    /// Unrealized profit/loss across all open positions.
    pub unrealized_pnl: f64,
    /// Portfolio change percentage for the current day.
    pub day_change_pct: f64,
}

impl Default for AccountState {
    fn default() -> Self {
        Self {
            equity: 0.0,
            cash: 0.0,
            unrealized_pnl: 0.0,
            day_change_pct: 0.0,
        }
    }
}

/// A single open position in the portfolio.
#[derive(Debug, Clone)]
pub struct PositionInfo {
    /// Trading instrument symbol (e.g. "BTCUSDT").
    pub symbol: String,
    /// Position direction ("Buy" or "Sell").
    pub side: String,
    /// Number of units held.
    pub quantity: f64,
    /// Average entry price.
    pub entry_price: f64,
    /// Current market price.
    pub current_price: f64,
    /// Unrealized profit/loss for this position.
    pub pnl: f64,
    /// Name of the strategy that opened this position.
    pub strategy: String,
}

/// A single order entry in the order log.
#[derive(Debug, Clone)]
pub struct OrderEntry {
    /// Timestamp when the order was placed (HH:MM:SS format).
    pub time: String,
    /// Trading instrument symbol.
    pub symbol: String,
    /// Order direction ("Buy" or "Sell").
    pub side: String,
    /// Order quantity.
    pub quantity: f64,
    /// Order price.
    pub price: f64,
    /// Order status: "Filled", "Rejected", or "Submitted".
    pub status: String,
    /// Name of the strategy that generated the order.
    pub strategy: String,
    /// Guard check result: "Pass" or rejection reason (e.g. "`MaxPos`").
    pub guard_result: String,
}

/// Time range for the `PnL` sparkline chart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Display, EnumString)]
pub enum PnlRange {
    /// Last 1 hour.
    #[strum(serialize = "1h")]
    Hour1,
    /// Last 4 hours.
    #[strum(serialize = "4h")]
    Hour4,
    /// Last 1 day.
    #[strum(serialize = "1d")]
    Day1,
    /// All available data.
    #[strum(serialize = "all")]
    All,
}

impl PnlRange {
    /// Cycle to the next time range: 1h -> 4h -> 1d -> all -> 1h.
    #[must_use]
    pub const fn next(self) -> Self {
        match self {
            Self::Hour1 => Self::Hour4,
            Self::Hour4 => Self::Day1,
            Self::Day1 => Self::All,
            Self::All => Self::Hour1,
        }
    }
}

/// State for the Trading tab.
#[derive(Debug, Clone)]
pub struct TradingState {
    /// Account-level summary.
    pub account: AccountState,
    /// Currently open positions.
    pub positions: Vec<PositionInfo>,
    /// Order log entries (most recent first).
    pub orders: Vec<OrderEntry>,
    /// `PnL` data points for the sparkline chart.
    pub pnl_data: Vec<u64>,
    /// Currently selected time range for the `PnL` sparkline.
    pub pnl_range: PnlRange,
    /// Index of the currently selected order in the orders list.
    pub selected_order: usize,
    /// Whether the order detail overlay is shown.
    pub show_order_detail: bool,
}

impl Default for TradingState {
    fn default() -> Self {
        Self {
            account: AccountState::default(),
            positions: Vec::new(),
            orders: Vec::new(),
            pnl_data: Vec::new(),
            pnl_range: PnlRange::Hour1,
            selected_order: 0,
            show_order_detail: false,
        }
    }
}

impl TradingState {
    /// Move selection down in the orders list.
    pub fn select_next_order(&mut self) {
        if !self.orders.is_empty() {
            self.selected_order = (self.selected_order + 1).min(self.orders.len() - 1);
        }
    }

    /// Move selection up in the orders list.
    pub const fn select_prev_order(&mut self) {
        self.selected_order = self.selected_order.saturating_sub(1);
    }

    /// Cycle the `PnL` sparkline time range.
    pub const fn cycle_pnl_range(&mut self) {
        self.pnl_range = self.pnl_range.next();
    }

    /// Toggle the order detail overlay.
    pub const fn toggle_order_detail(&mut self) {
        self.show_order_detail = !self.show_order_detail;
    }
}

/// Index of the Strategies tab in the tab bar.
pub const STRATEGIES_TAB: usize = 3;

/// Lifecycle status of a strategy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StrategyLifecycle {
    /// Strategy has been promoted to live trading.
    Promoted,
    /// Strategy is actively being evaluated.
    Active,
    /// Strategy was demoted from live trading.
    Demoted,
    /// Strategy has been retired permanently.
    Retired,
    /// Strategy is archived for historical reference.
    Archived,
}

/// A single strategy entry displayed in the strategy list table.
#[derive(Debug, Clone)]
pub struct StrategyEntry {
    /// Display name of the strategy.
    pub name: String,
    /// Version number of the strategy.
    pub version: u32,
    /// Current lifecycle status.
    pub status: StrategyLifecycle,
    /// Sharpe ratio from the most recent evaluation.
    pub sharpe: f64,
    /// Maximum drawdown percentage (negative value).
    pub max_drawdown: f64,
    /// Total number of trades executed.
    pub trade_count: u32,
    /// Date of the last evaluation, if any.
    pub last_eval: Option<String>,
    /// The original hypothesis that spawned this strategy.
    pub origin_hypothesis: String,
    /// Date the strategy was created.
    pub created_at: String,
    /// Date the strategy was promoted to live, if ever.
    pub promoted_at: Option<String>,
}

/// A single evaluation record in the evaluation timeline.
#[derive(Debug, Clone)]
pub struct EvaluationEntry {
    /// Timestamp of the evaluation.
    pub time: String,
    /// Number of trades in the evaluation window.
    pub trades: u32,
    /// Sharpe ratio for this evaluation period.
    pub sharpe: f64,
    /// Drawdown percentage during this evaluation period.
    pub drawdown: f64,
    /// Decision made (Promote, Demote, Retire, Hold).
    pub decision: String,
    /// Human-readable reason for the decision.
    pub reason: String,
}

/// State for the Strategies tab, managing list selection and detail expansion.
pub struct StrategiesState {
    /// All strategy entries to display.
    pub strategies: Vec<StrategyEntry>,
    /// Index of the currently selected strategy in the list.
    pub selected_index: usize,
    /// Evaluation history for the selected strategy.
    pub evaluations: Vec<EvaluationEntry>,
    /// Whether the full hypothesis text is shown in the detail panel.
    pub show_detail: bool,
    /// Whether the DAG popup is visible.
    pub show_dag: bool,
}

impl StrategiesState {
    /// Create a new strategies state populated with sample data.
    #[must_use]
    pub fn new_with_samples() -> Self {
        Self {
            strategies: sample_strategies(),
            selected_index: 0,
            evaluations: sample_evaluations(),
            show_detail: false,
            show_dag: false,
        }
    }

    /// Move selection up by one row, clamping at the top.
    pub const fn select_previous(&mut self) {
        self.selected_index = self.selected_index.saturating_sub(1);
    }

    /// Move selection down by one row, clamping at the bottom.
    pub fn select_next(&mut self) {
        if !self.strategies.is_empty() {
            self.selected_index = (self.selected_index + 1).min(self.strategies.len() - 1);
        }
    }

    /// Toggle the detail expansion for the selected strategy.
    pub const fn toggle_detail(&mut self) {
        self.show_detail = !self.show_detail;
    }

    /// Toggle the DAG popup.
    pub const fn toggle_dag(&mut self) {
        self.show_dag = !self.show_dag;
    }
}

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
    /// State for the Research tab.
    pub research: ResearchState,
    /// State for the Events tab.
    pub events_state: EventsState,
    /// State for the Trading tab.
    pub trading: TradingState,
    /// State for the Strategies tab.
    pub strategies_state: StrategiesState,
}

impl App {
    /// Create a new app instance targeting the given gRPC server address.
    #[must_use]
    pub fn new(server_addr: String) -> Self {
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
            research: ResearchState::empty(),
            events_state: EventsState::default(),
            trading: TradingState::default(),
            strategies_state: StrategiesState::new_with_samples(),
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

/// Generate sample strategy entries for development/demo purposes.
fn sample_strategies() -> Vec<StrategyEntry> {
    vec![
        StrategyEntry {
            name: "MeanRev-BTC-4h".to_string(),
            version: 3,
            status: StrategyLifecycle::Promoted,
            sharpe: 1.42,
            max_drawdown: -8.3,
            trade_count: 142,
            last_eval: Some("2026-03-25".to_string()),
            origin_hypothesis: "BTC mean reversion on 4h RSI oversold conditions with volume confirmation shows consistent alpha in ranging markets".to_string(),
            created_at: "2026-01-15".to_string(),
            promoted_at: Some("2026-02-10".to_string()),
        },
        StrategyEntry {
            name: "Momentum-ETH-1h".to_string(),
            version: 2,
            status: StrategyLifecycle::Active,
            sharpe: 1.18,
            max_drawdown: -11.2,
            trade_count: 89,
            last_eval: Some("2026-03-24".to_string()),
            origin_hypothesis: "ETH momentum breakout on 1h timeframe using VWAP crossover".to_string(),
            created_at: "2026-02-01".to_string(),
            promoted_at: None,
        },
        StrategyEntry {
            name: "StatArb-SOL".to_string(),
            version: 1,
            status: StrategyLifecycle::Demoted,
            sharpe: 0.91,
            max_drawdown: -15.7,
            trade_count: 67,
            last_eval: Some("2026-03-20".to_string()),
            origin_hypothesis: "SOL statistical arbitrage against BTC using cointegration z-score".to_string(),
            created_at: "2026-01-20".to_string(),
            promoted_at: Some("2026-02-15".to_string()),
        },
        StrategyEntry {
            name: "GridBot-BNB".to_string(),
            version: 4,
            status: StrategyLifecycle::Retired,
            sharpe: 0.45,
            max_drawdown: -22.1,
            trade_count: 312,
            last_eval: Some("2026-03-10".to_string()),
            origin_hypothesis: "BNB grid trading in stable range-bound periods".to_string(),
            created_at: "2025-11-05".to_string(),
            promoted_at: Some("2025-12-01".to_string()),
        },
        StrategyEntry {
            name: "DCA-Weekly".to_string(),
            version: 1,
            status: StrategyLifecycle::Archived,
            sharpe: 0.62,
            max_drawdown: -18.4,
            trade_count: 52,
            last_eval: None,
            origin_hypothesis: "Weekly dollar cost averaging with volatility-adjusted sizing".to_string(),
            created_at: "2025-09-01".to_string(),
            promoted_at: None,
        },
    ]
}

/// Generate sample evaluation entries for development/demo purposes.
fn sample_evaluations() -> Vec<EvaluationEntry> {
    vec![
        EvaluationEntry {
            time: "2026-03-25".to_string(),
            trades: 42,
            sharpe: 1.42,
            drawdown: -8.3,
            decision: "Promote".to_string(),
            reason: "Consistent alpha over 30-day window".to_string(),
        },
        EvaluationEntry {
            time: "2026-03-18".to_string(),
            trades: 38,
            sharpe: 1.31,
            drawdown: -9.1,
            decision: "Hold".to_string(),
            reason: "Needs more data for confidence".to_string(),
        },
        EvaluationEntry {
            time: "2026-03-11".to_string(),
            trades: 35,
            sharpe: 1.15,
            drawdown: -10.2,
            decision: "Hold".to_string(),
            reason: "Market regime shift detected".to_string(),
        },
        EvaluationEntry {
            time: "2026-03-04".to_string(),
            trades: 27,
            sharpe: 0.88,
            drawdown: -12.5,
            decision: "Demote".to_string(),
            reason: "Drawdown exceeds threshold".to_string(),
        },
    ]
}
