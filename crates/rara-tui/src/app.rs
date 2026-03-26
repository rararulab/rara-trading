//! Application state for the TUI dashboard.

use rara_server::rara_proto::SystemStatus;

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
