//! Terminal event loop driving the TUI application.
//!
//! Handles crossterm input events, periodic status polling via gRPC, and
//! terminal setup/teardown.

use std::io;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use snafu::ResultExt;
use tonic::transport::Channel;
use tracing::{info, warn};

use rara_server::rara_proto::rara_service_client::RaraServiceClient;
use rara_server::rara_proto::Empty;

use crate::app::{App, ConnectionStatus, EventFilter, EVENTS_TAB_INDEX, TAB_RESEARCH};
use crate::error::{IoSnafu, Result};
use crate::tabs;
use crate::ui;

/// Duration between status poll ticks.
const TICK_RATE: Duration = Duration::from_millis(1000);

/// Duration for crossterm event polling.
const POLL_TIMEOUT: Duration = Duration::from_millis(100);

/// Run the TUI event loop, connecting to the given gRPC server address.
///
/// This function takes ownership of the terminal for the duration of the
/// application. On exit (or panic), the terminal is restored.
pub async fn run(server_addr: &str) -> Result<()> {
    // Terminal setup
    enable_raw_mode().context(IoSnafu)?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen).context(IoSnafu)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).context(IoSnafu)?;

    let mut app = App::new(server_addr.to_string());

    // Try initial connection
    let mut client = try_connect(server_addr).await;
    if client.is_some() {
        app.connection_status = ConnectionStatus::Connected;
    }

    let mut last_poll = std::time::Instant::now();

    let result = event_loop(&mut terminal, &mut app, &mut client, &mut last_poll).await;

    // Terminal teardown — always runs even on error
    disable_raw_mode().context(IoSnafu)?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen).context(IoSnafu)?;
    terminal.show_cursor().context(IoSnafu)?;

    result
}

/// Core event loop: poll input, tick status, render.
async fn event_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    client: &mut Option<RaraServiceClient<Channel>>,
    last_poll: &mut std::time::Instant,
) -> Result<()> {
    while app.running {
        // Render
        terminal.draw(|frame| ui::render(frame, app)).context(IoSnafu)?;

        // Poll crossterm events (non-blocking with timeout)
        if event::poll(POLL_TIMEOUT).context(IoSnafu)?
            && let Event::Key(key) = event::read().context(IoSnafu)?
            && key.kind == KeyEventKind::Press
        {
            handle_key(app, key.code);
        }

        // Periodic status poll
        if last_poll.elapsed() >= TICK_RATE {
            *last_poll = std::time::Instant::now();
            poll_status(app, client).await?;
        }
    }

    Ok(())
}

/// Handle a key press event, dispatching to tab-specific handlers when needed.
fn handle_key(app: &mut App, key: KeyCode) {
    // Research tab DAG popup intercepts Esc to close instead of quitting
    if app.active_tab == TAB_RESEARCH && app.research.show_dag {
        match key {
            KeyCode::Esc | KeyCode::Char('p') => app.research.close_dag(),
            KeyCode::Char('q') => app.quit(),
            _ => {}
        }
        return;
    }

    // When search is active on the events tab, capture all input for search
    if app.active_tab == EVENTS_TAB_INDEX && app.events_state.search_active {
        handle_events_search_key(app, key);
        return;
    }

    match key {
        KeyCode::Char('q') | KeyCode::Esc => app.quit(),
        KeyCode::Char('1') => app.select_tab(0),
        KeyCode::Char('2') => app.select_tab(1),
        KeyCode::Char('3') => app.select_tab(2),
        KeyCode::Char('4') => app.select_tab(3),
        KeyCode::Char('5') => app.select_tab(4),
        _ if app.active_tab == EVENTS_TAB_INDEX => handle_events_key(app, key),
        _ if app.active_tab == TAB_RESEARCH => handle_research_key(app, key),
        _ => {}
    }
}

/// Handle key presses specific to the Events tab (non-search mode).
fn handle_events_key(app: &mut App, key: KeyCode) {
    let state = &mut app.events_state;
    match key {
        // Topic filters
        KeyCode::Char('a') => {
            state.filter = EventFilter::All;
            state.selected_index = 0;
        }
        KeyCode::Char('t') => {
            state.filter = EventFilter::Trading;
            state.selected_index = 0;
        }
        KeyCode::Char('r') => {
            state.filter = EventFilter::Research;
            state.selected_index = 0;
        }
        KeyCode::Char('f') => {
            state.filter = EventFilter::Feedback;
            state.selected_index = 0;
        }
        KeyCode::Char('s') => {
            state.filter = EventFilter::Sentinel;
            state.selected_index = 0;
        }
        // Pause/resume auto-scroll
        KeyCode::Char(' ') => {
            state.auto_scroll = !state.auto_scroll;
        }
        // Navigation (when paused)
        KeyCode::Char('j') | KeyCode::Down => {
            state.auto_scroll = false;
            let count = tabs::events::filtered_count(state);
            if count > 0 && state.selected_index < count - 1 {
                state.selected_index += 1;
            }
        }
        KeyCode::Char('k') | KeyCode::Up => {
            state.auto_scroll = false;
            if state.selected_index > 0 {
                state.selected_index -= 1;
            }
        }
        // Jump to latest
        KeyCode::Char('G') => {
            let count = tabs::events::filtered_count(state);
            if count > 0 {
                state.selected_index = count - 1;
            }
            state.auto_scroll = true;
        }
        // Enter search mode
        KeyCode::Char('/') => {
            state.search_active = true;
            state.search_query.clear();
        }
        // Toggle detail pane
        KeyCode::Enter => {
            state.detail_expanded = !state.detail_expanded;
        }
        _ => {}
    }
}

/// Handle key presses while search input is active on the Events tab.
fn handle_events_search_key(app: &mut App, key: KeyCode) {
    let state = &mut app.events_state;
    match key {
        KeyCode::Esc => {
            state.search_active = false;
            state.search_query.clear();
            state.selected_index = 0;
        }
        KeyCode::Enter => {
            state.search_active = false;
            state.selected_index = 0;
        }
        KeyCode::Backspace => {
            state.search_query.pop();
        }
        KeyCode::Char(c) => {
            state.search_query.push(c);
        }
        _ => {}
    }
}

/// Handle key presses specific to the Research tab.
fn handle_research_key(app: &mut App, key: KeyCode) {
    match key {
        KeyCode::Char('j') | KeyCode::Down => app.research.select_next(),
        KeyCode::Char('k') | KeyCode::Up => app.research.select_prev(),
        KeyCode::Char('p') => app.research.toggle_dag(),
        _ => {}
    }
}

/// Poll the gRPC server for system status, handling reconnection on failure.
async fn poll_status(
    app: &mut App,
    client: &mut Option<RaraServiceClient<Channel>>,
) -> Result<()> {
    if let Some(c) = client.as_mut() {
        match c.get_system_status(Empty {}).await {
            Ok(response) => {
                app.system_status = Some(response.into_inner());
                app.connection_status = ConnectionStatus::Connected;
            }
            Err(status) => {
                warn!("gRPC status poll failed: {status}");
                let retry_count = match &app.connection_status {
                    ConnectionStatus::Disconnected { retry_count } => retry_count + 1,
                    _ => 1,
                };
                app.connection_status = ConnectionStatus::Disconnected { retry_count };
                *client = None;
            }
        }
    } else {
        // Try to reconnect
        let retry_count = match &app.connection_status {
            ConnectionStatus::Disconnected { retry_count } => *retry_count,
            _ => 0,
        };

        if let Some(c) = try_connect(&app.server_addr).await {
            info!("reconnected to gRPC server");
            *client = Some(c);
            app.connection_status = ConnectionStatus::Connected;
        } else {
            app.connection_status = ConnectionStatus::Disconnected {
                retry_count: retry_count + 1,
            };
        }
    }

    Ok(())
}

/// Attempt a non-blocking gRPC connection. Returns `None` on failure.
async fn try_connect(addr: &str) -> Option<RaraServiceClient<Channel>> {
    let endpoint = Channel::from_shared(addr.to_string()).ok()?;
    let channel = tokio::time::timeout(Duration::from_secs(2), endpoint.connect())
        .await
        .ok()?
        .ok()?;
    Some(RaraServiceClient::new(channel))
}
