//! Application state for the TUI dashboard.

use std::path::PathBuf;

use rara_research::strategy_registry::{FetchedStrategy, StrategyRegistry};
use rara_server::rara_proto::SystemStatus;
use strum::{Display, EnumString};
use tracing::warn;

use crate::tabs::research::ResearchState;

/// Live status of a running strategy.
#[derive(Debug, Clone)]
pub struct StrategyStatus {
    /// Human-readable strategy name.
    pub name:   String,
    /// Current lifecycle state (e.g. "Running", "Promoted", "Paper",
    /// "Stopped").
    pub status: String,
    /// Cumulative profit-and-loss.
    pub pnl:    f64,
    /// Annualized Sharpe ratio.
    pub sharpe: f64,
}

/// A single event for the recent-events feed.
#[derive(Debug, Clone)]
pub struct RecentEvent {
    /// Formatted timestamp (e.g. "14:32:01").
    pub time:       String,
    /// Event category tag (e.g. "TRADE", "ERROR", "INFO").
    pub event_type: String,
    /// One-line event description.
    pub summary:    String,
}

/// Aggregate progress of the autonomous research pipeline.
#[derive(Debug, Clone)]
pub struct ResearchProgress {
    /// Number of backtests completed so far.
    pub current:     u32,
    /// Total backtests planned for this sweep.
    pub total:       u32,
    /// Strategies that passed acceptance criteria.
    pub accepted:    u32,
    /// Strategies that failed acceptance criteria.
    pub rejected:    u32,
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
    pub seq:         u64,
    /// Human-readable timestamp string.
    pub time:        String,
    /// Event topic (trading, research, feedback, sentinel).
    pub topic:       String,
    /// The type/kind of event within the topic.
    pub event_type:  String,
    /// One-line summary of the event.
    pub summary:     String,
    /// Optional strategy identifier associated with this event.
    pub strategy_id: Option<String>,
    /// Full event payload, typically JSON.
    pub payload:     String,
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
    pub events:          Vec<EventEntry>,
    /// Current topic filter.
    pub filter:          EventFilter,
    /// Whether auto-scroll is enabled (newest events stay visible).
    pub auto_scroll:     bool,
    /// Currently selected row index within the filtered view.
    pub selected_index:  usize,
    /// Current search query text.
    pub search_query:    String,
    /// Whether the search input is actively accepting keystrokes.
    pub search_active:   bool,
    /// Whether the detail pane is expanded.
    pub detail_expanded: bool,
}

impl Default for EventsState {
    fn default() -> Self {
        Self {
            events:          Vec::new(),
            filter:          EventFilter::All,
            auto_scroll:     true,
            selected_index:  0,
            search_query:    String::new(),
            search_active:   false,
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
    pub equity:         f64,
    /// Available cash balance.
    pub cash:           f64,
    /// Unrealized profit/loss across all open positions.
    pub unrealized_pnl: f64,
    /// Portfolio change percentage for the current day.
    pub day_change_pct: f64,
}

impl Default for AccountState {
    fn default() -> Self {
        Self {
            equity:         0.0,
            cash:           0.0,
            unrealized_pnl: 0.0,
            day_change_pct: 0.0,
        }
    }
}

/// A single open position in the portfolio.
#[derive(Debug, Clone)]
pub struct PositionInfo {
    /// Trading instrument symbol (e.g. "BTCUSDT").
    pub symbol:        String,
    /// Position direction ("Buy" or "Sell").
    pub side:          String,
    /// Number of units held.
    pub quantity:      f64,
    /// Average entry price.
    pub entry_price:   f64,
    /// Current market price.
    pub current_price: f64,
    /// Unrealized profit/loss for this position.
    pub pnl:           f64,
    /// Name of the strategy that opened this position.
    pub strategy:      String,
}

/// A single order entry in the order log.
#[derive(Debug, Clone)]
pub struct OrderEntry {
    /// Timestamp when the order was placed (HH:MM:SS format).
    pub time:         String,
    /// Trading instrument symbol.
    pub symbol:       String,
    /// Order direction ("Buy" or "Sell").
    pub side:         String,
    /// Order quantity.
    pub quantity:     f64,
    /// Order price.
    pub price:        f64,
    /// Order status: "Filled", "Rejected", or "Submitted".
    pub status:       String,
    /// Name of the strategy that generated the order.
    pub strategy:     String,
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
    pub account:           AccountState,
    /// Currently open positions.
    pub positions:         Vec<PositionInfo>,
    /// Order log entries (most recent first).
    pub orders:            Vec<OrderEntry>,
    /// `PnL` data points for the sparkline chart.
    pub pnl_data:          Vec<u64>,
    /// Currently selected time range for the `PnL` sparkline.
    pub pnl_range:         PnlRange,
    /// Index of the currently selected order in the orders list.
    pub selected_order:    usize,
    /// Whether the order detail overlay is shown.
    pub show_order_detail: bool,
}

impl Default for TradingState {
    fn default() -> Self {
        Self {
            account:           AccountState::default(),
            positions:         Vec::new(),
            orders:            Vec::new(),
            pnl_data:          Vec::new(),
            pnl_range:         PnlRange::Hour1,
            selected_order:    0,
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
    pub const fn cycle_pnl_range(&mut self) { self.pnl_range = self.pnl_range.next(); }

    /// Toggle the order detail overlay.
    pub const fn toggle_order_detail(&mut self) {
        self.show_order_detail = !self.show_order_detail;
    }
}

/// Index of the Strategies tab in the tab bar.
pub const STRATEGIES_TAB: usize = 3;

/// Lifecycle status of a strategy.
#[derive(Debug, Clone, PartialEq, Eq, Display, EnumString)]
pub enum StrategyLifecycle {
    /// Strategy is installed and available for use.
    #[strum(serialize = "Installed")]
    Installed,
}

/// A single strategy entry displayed in the strategy list table.
///
/// Populated from real installed strategies via
/// `StrategyRegistry::list_installed()`.
#[derive(Debug, Clone)]
pub struct StrategyEntry {
    /// Display name of the strategy.
    pub name:            String,
    /// Version number of the strategy (from WASM metadata).
    pub version:         u32,
    /// Release tag from the GitHub registry.
    pub tag:             String,
    /// Release version string (e.g. "v0.1.0").
    pub release_version: String,
    /// API version the strategy was compiled against.
    pub api_version:     u32,
    /// Brief description from the WASM metadata.
    pub description:     String,
    /// Current lifecycle status.
    pub status:          StrategyLifecycle,
    /// Local filesystem path to the WASM binary.
    pub wasm_path:       PathBuf,
    /// WASM file size in bytes.
    pub file_size:       u64,
    /// WASM asset download URL from the registry.
    pub wasm_url:        String,
}

/// State for the Strategies tab, managing list selection and detail expansion.
pub struct StrategiesState {
    /// All strategy entries to display.
    pub strategies:     Vec<StrategyEntry>,
    /// Index of the currently selected strategy in the list.
    pub selected_index: usize,
    /// Whether the full description is shown in the detail panel.
    pub show_detail:    bool,
}

impl StrategiesState {
    /// Create a new strategies state by reading installed strategies from disk.
    ///
    /// Reads `.registry.json` files from the promoted strategies directory.
    /// If no strategies are installed or reading fails, returns an empty list.
    #[must_use]
    pub fn from_installed(promoted_dir: PathBuf) -> Self {
        let strategies = load_installed_strategies(promoted_dir);
        Self {
            strategies,
            selected_index: 0,
            show_detail: false,
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
    pub const fn toggle_detail(&mut self) { self.show_detail = !self.show_detail; }
}

/// Phase of the TUI application lifecycle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppPhase {
    /// Waiting for the gRPC server to become ready.
    StartingServer {
        /// Status message to display.
        message:  String,
        /// Number of connection attempts so far.
        attempts: u32,
    },
    /// Server is ready; show the main dashboard.
    Ready,
}

/// Root application state driving the TUI.
pub struct App {
    /// Index of the currently active tab.
    pub active_tab:        usize,
    /// Whether the application is still running.
    pub running:           bool,
    /// Current connection state to the gRPC server.
    pub connection_status: ConnectionStatus,
    /// Last received system status from the server.
    pub system_status:     Option<SystemStatus>,
    /// gRPC server address the client connects to.
    pub server_addr:       String,
    /// Live strategy statuses displayed on the overview tab.
    pub strategies:        Vec<StrategyStatus>,
    /// Open positions displayed on the overview tab.
    pub positions:         Vec<PositionInfo>,
    /// Rolling window of recent events.
    pub recent_events:     Vec<RecentEvent>,
    /// Active alert messages.
    pub alerts:            Vec<String>,
    /// Current research pipeline progress, if any.
    pub research_progress: Option<ResearchProgress>,
    /// State for the Research tab.
    pub research:          ResearchState,
    /// State for the Events tab.
    pub events_state:      EventsState,
    /// State for the Trading tab.
    pub trading:           TradingState,
    /// State for the Strategies tab.
    pub strategies_state:  StrategiesState,
    /// Current lifecycle phase of the TUI application.
    pub phase:             AppPhase,
}

impl App {
    /// Create a new app instance targeting the given gRPC server address.
    ///
    /// Loads installed strategies from the promoted directory on disk.
    #[must_use]
    pub fn new(server_addr: String, promoted_dir: PathBuf) -> Self {
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
            strategies_state: StrategiesState::from_installed(promoted_dir),
            phase: AppPhase::StartingServer {
                message:  "Starting gRPC server...".to_string(),
                attempts: 0,
            },
        }
    }

    /// Switch to a tab by index (0-based). Out-of-range values are ignored.
    pub const fn select_tab(&mut self, index: usize) {
        if index < TAB_NAMES.len() {
            self.active_tab = index;
        }
    }

    /// Signal the application to quit.
    pub const fn quit(&mut self) { self.running = false; }
}

/// Convert a `FetchedStrategy` from the registry into a TUI view model.
fn fetched_to_entry(fetched: FetchedStrategy) -> StrategyEntry {
    StrategyEntry {
        name:            fetched.meta.name,
        version:         fetched.meta.version,
        tag:             fetched.entry.tag,
        release_version: fetched.entry.version,
        api_version:     fetched.meta.api_version,
        description:     fetched.meta.description,
        status:          StrategyLifecycle::Installed,
        wasm_path:       fetched.wasm_path,
        file_size:       fetched.entry.size,
        wasm_url:        fetched.entry.wasm_url,
    }
}

/// Load installed strategies from the promoted directory on disk.
///
/// Returns an empty vec if the directory doesn't exist or reading fails.
fn load_installed_strategies(promoted_dir: PathBuf) -> Vec<StrategyEntry> {
    let registry = StrategyRegistry::builder()
        .promoted_dir(promoted_dir)
        .build();

    match registry.list_installed() {
        Ok(strategies) => strategies.into_iter().map(fetched_to_entry).collect(),
        Err(err) => {
            warn!(%err, "failed to load installed strategies");
            Vec::new()
        }
    }
}

/// Maximum number of events to retain in the events buffer.
const MAX_EVENTS: usize = 1000;

/// Maximum number of recent events shown on the overview tab.
const MAX_RECENT_EVENTS: usize = 50;

impl App {
    /// Ingest a gRPC [`EventMessage`] into the application state.
    ///
    /// Converts the message into an [`EventEntry`] for the Events tab and a
    /// [`RecentEvent`] for the Overview tab. Older events are evicted when the
    /// buffer exceeds [`MAX_EVENTS`].
    pub fn push_event(&mut self, msg: rara_server::rara_proto::EventMessage) {
        // Derive topic from the dotted event_type (e.g. "trading.order.filled" ->
        // "trading")
        let topic = msg
            .event_type
            .split('.')
            .next()
            .unwrap_or("unknown")
            .to_string();

        // Build a one-line summary from the payload JSON (first 80 chars)
        let summary = build_summary(&msg.payload_json);

        let strategy_id = if msg.strategy_id.is_empty() {
            None
        } else {
            Some(msg.strategy_id.clone())
        };

        // Truncate timestamp to HH:MM:SS for display
        let time = truncate_timestamp(&msg.timestamp);

        let entry = EventEntry {
            seq: msg.sequence,
            time: time.clone(),
            topic: topic.clone(),
            event_type: msg.event_type.clone(),
            summary: summary.clone(),
            strategy_id,
            payload: msg.payload_json,
        };

        self.events_state.events.push(entry);

        // Evict oldest events when buffer is full
        if self.events_state.events.len() > MAX_EVENTS {
            let drain_count = self.events_state.events.len() - MAX_EVENTS;
            self.events_state.events.drain(..drain_count);
        }

        // Auto-scroll: keep selected_index at the end
        if self.events_state.auto_scroll && !self.events_state.events.is_empty() {
            let filtered_len = crate::tabs::events::filtered_count(&self.events_state);
            if filtered_len > 0 {
                self.events_state.selected_index = filtered_len - 1;
            }
        }

        // Mirror to recent_events for the Overview tab
        let event_type_tag = topic.to_uppercase();
        self.recent_events.push(RecentEvent {
            time,
            event_type: event_type_tag,
            summary,
        });
        if self.recent_events.len() > MAX_RECENT_EVENTS {
            let drain_count = self.recent_events.len() - MAX_RECENT_EVENTS;
            self.recent_events.drain(..drain_count);
        }
    }
}

/// Extract a short display timestamp from an ISO-8601 or similar string.
///
/// Looks for a `T` separator and takes the time portion up to the first dot
/// (fractional seconds) or end. Falls back to the first 8 characters.
fn truncate_timestamp(ts: &str) -> String {
    ts.split('T')
        .nth(1)
        .unwrap_or(ts)
        .split('.')
        .next()
        .unwrap_or(ts)
        .chars()
        .take(8)
        .collect()
}

/// Build a one-line summary from a JSON payload string.
///
/// Attempts to extract a "message" or "msg" field; otherwise truncates the
/// raw JSON to 80 characters.
fn build_summary(payload_json: &str) -> String {
    // Quick extraction without full JSON parse for performance
    for key in &["\"message\":", "\"msg\":"] {
        if let Some(pos) = payload_json.find(key) {
            let after = &payload_json[pos + key.len()..];
            let trimmed = after.trim_start();
            if let Some(inner) = trimmed.strip_prefix('"') {
                // Extract string value between quotes
                if let Some(end) = inner.find('"') {
                    return inner[..end].to_string();
                }
            }
        }
    }
    // Fallback: truncate raw payload
    let truncated: String = payload_json.chars().take(80).collect();
    if payload_json.len() > 80 {
        format!("{truncated}...")
    } else {
        truncated
    }
}
