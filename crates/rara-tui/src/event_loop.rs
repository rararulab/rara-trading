//! Terminal event loop driving the TUI application.
//!
//! Handles crossterm input events, periodic status polling via gRPC,
//! real-time event streaming via `StreamEvents`, and terminal
//! setup/teardown.

use std::{io, path::PathBuf, time::Duration};

use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use rara_server::rara_proto::{
    Empty, EventFilter as ProtoEventFilter, EventMessage, rara_service_client::RaraServiceClient,
};
use ratatui::{Terminal, backend::CrosstermBackend};
use snafu::ResultExt;
use tokio::sync::mpsc;
use tonic::transport::Channel;
use tracing::{info, warn};

use crate::{
    app::{
        App, AppPhase, ConnectionStatus, EVENTS_TAB_INDEX, EventFilter, STRATEGIES_TAB,
        TAB_RESEARCH, TRADING_TAB,
    },
    error::{IoSnafu, Result},
    server_process::ServerProcess,
    tabs, ui,
};

/// Duration between status poll ticks.
const TICK_RATE: Duration = Duration::from_millis(1000);

/// Duration for crossterm event polling.
const POLL_TIMEOUT: Duration = Duration::from_millis(100);

/// Maximum connection attempts before giving up during startup.
const MAX_STARTUP_ATTEMPTS: u32 = 60;

/// Channel buffer size for streamed events.
const EVENT_CHANNEL_SIZE: usize = 256;

/// Run the TUI event loop.
///
/// When `server_addr` is `Some`, connects to an existing gRPC server.
/// When `None` (standalone mode), auto-spawns a `rara-trading serve` child
/// process on a random available port and connects to it.
///
/// This function takes ownership of the terminal for the duration of the
/// application. On exit (or panic), the terminal is restored and any spawned
/// server subprocess is killed.
pub async fn run(server_addr: Option<&str>, promoted_dir: PathBuf) -> Result<()> {
    // In standalone mode, spawn a server subprocess
    let mut server_process = if server_addr.is_some() {
        None
    } else {
        info!("no --server provided, spawning gRPC server subprocess (standalone mode)");
        Some(ServerProcess::spawn().await?)
    };

    let effective_addr = match (&server_process, server_addr) {
        (Some(proc), _) => proc.server_addr(),
        (_, Some(addr)) => addr.to_string(),
        _ => unreachable!(),
    };
    // Terminal setup
    enable_raw_mode().context(IoSnafu)?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen).context(IoSnafu)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).context(IoSnafu)?;

    let mut app = App::new(effective_addr.clone(), promoted_dir);

    let mut client: Option<RaraServiceClient<Channel>> = None;

    let mut last_poll = std::time::Instant::now();

    let result = event_loop(&mut terminal, &mut app, &mut client, &mut last_poll).await;

    // Terminal teardown — always runs even on error
    disable_raw_mode().context(IoSnafu)?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen).context(IoSnafu)?;
    terminal.show_cursor().context(IoSnafu)?;

    // Shutdown spawned server subprocess if we started one
    if let Some(proc) = server_process.as_mut() {
        proc.shutdown().await;
    }

    result
}

/// Core event loop: poll input, tick status, receive streamed events, render.
///
/// During the startup phase, only quit keys are accepted and the loop
/// periodically attempts to connect to the gRPC server with a health check.
/// Once the server is confirmed ready, transitions to the normal dashboard
/// phase and spawns the event stream subscriber.
async fn event_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    client: &mut Option<RaraServiceClient<Channel>>,
    last_poll: &mut std::time::Instant,
) -> Result<()> {
    // Channel for receiving streamed events from the background task
    let (event_tx, mut event_rx) = mpsc::channel::<EventMessage>(EVENT_CHANNEL_SIZE);
    // Track the stream task so we can detect when it dies and respawn
    let mut stream_task: Option<tokio::task::JoinHandle<()>> = None;

    while app.running {
        // Render
        terminal
            .draw(|frame| ui::render(frame, app))
            .context(IoSnafu)?;

        // Poll crossterm events (non-blocking with timeout)
        if event::poll(POLL_TIMEOUT).context(IoSnafu)?
            && let Event::Key(key) = event::read().context(IoSnafu)?
            && key.kind == KeyEventKind::Press
        {
            // During startup, only allow quit
            match &app.phase {
                AppPhase::StartingServer { .. } => {
                    if matches!(key.code, KeyCode::Char('q') | KeyCode::Esc) {
                        app.quit();
                    }
                }
                AppPhase::Ready => handle_key(app, key.code),
            }
        }

        // Drain any streamed events from the background task
        while let Ok(msg) = event_rx.try_recv() {
            app.push_event(msg);
        }

        // Check if the stream task has died and needs respawning
        if let Some(handle) = &stream_task
            && handle.is_finished()
        {
            warn!("event stream task ended, will respawn on next status poll");
            stream_task = None;
        }

        // Startup phase: try to connect to gRPC server
        if let AppPhase::StartingServer { attempts, .. } = &app.phase {
            let attempts = *attempts;

            // Give up after MAX_STARTUP_ATTEMPTS (~30s at 500ms intervals)
            if attempts >= MAX_STARTUP_ATTEMPTS {
                warn!("gRPC server did not become ready after {attempts} attempts, giving up");
                app.phase = AppPhase::StartingServer {
                    message: format!(
                        "Server failed to start after {MAX_STARTUP_ATTEMPTS} attempts. Press q to \
                         quit."
                    ),
                    attempts,
                };
                // Render one final frame, then just handle quit keys
                terminal
                    .draw(|frame| ui::render(frame, app))
                    .context(IoSnafu)?;
                loop {
                    if event::poll(POLL_TIMEOUT).context(IoSnafu)?
                        && let Event::Key(key) = event::read().context(IoSnafu)?
                        && key.kind == KeyEventKind::Press
                        && matches!(key.code, KeyCode::Char('q') | KeyCode::Esc)
                    {
                        app.quit();
                        break;
                    }
                }
                continue;
            }

            if last_poll.elapsed() >= Duration::from_millis(500) {
                *last_poll = std::time::Instant::now();
                if let Some(mut c) = try_connect(&app.server_addr).await {
                    // Verify server is actually responding with a health check
                    if c.get_system_status(Empty {}).await.is_ok() {
                        info!("gRPC server is ready");
                        app.connection_status = ConnectionStatus::Connected;
                        app.phase = AppPhase::Ready;

                        // Spawn event stream subscriber
                        stream_task = Some(spawn_event_stream(c.clone(), event_tx.clone()));

                        *client = Some(c);
                    } else {
                        app.phase = AppPhase::StartingServer {
                            message:  "Server connected, waiting for ready...".to_string(),
                            attempts: attempts + 1,
                        };
                    }
                } else {
                    app.phase = AppPhase::StartingServer {
                        message:  "Connecting to gRPC server...".to_string(),
                        attempts: attempts + 1,
                    };
                }
            }
            continue;
        }

        // Normal phase: periodic status poll
        if last_poll.elapsed() >= TICK_RATE {
            *last_poll = std::time::Instant::now();
            poll_status(app, client).await?;

            // Respawn event stream if it died and we have a live client
            if stream_task.is_none()
                && let Some(c) = client.as_ref()
            {
                info!("respawning event stream subscriber");
                stream_task = Some(spawn_event_stream(c.clone(), event_tx.clone()));
            }
        }
    }

    // Cancel the stream task on exit
    if let Some(handle) = stream_task.take() {
        handle.abort();
    }

    Ok(())
}

/// Spawn a background task that subscribes to the gRPC event stream.
///
/// Received [`EventMessage`]s are forwarded through `tx`. The task runs
/// until the stream ends, the channel is closed, or an error occurs.
fn spawn_event_stream(
    mut client: RaraServiceClient<Channel>,
    tx: mpsc::Sender<EventMessage>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let request = ProtoEventFilter {
            topic: String::new(), // subscribe to all topics
        };

        let stream_result = client.stream_events(request).await;
        let mut stream = match stream_result {
            Ok(response) => response.into_inner(),
            Err(status) => {
                warn!("failed to start event stream: {status}");
                return;
            }
        };

        info!("event stream connected, receiving events");

        loop {
            match stream.message().await {
                Ok(Some(msg)) => {
                    if tx.send(msg).await.is_err() {
                        // Receiver dropped — TUI is shutting down
                        break;
                    }
                }
                Ok(None) => {
                    info!("event stream ended (server closed)");
                    break;
                }
                Err(status) => {
                    warn!("event stream error: {status}");
                    break;
                }
            }
        }
    })
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

    // When order detail overlay is shown, Esc/Enter closes it
    if app.active_tab == TRADING_TAB && app.trading.show_order_detail {
        match key {
            KeyCode::Esc | KeyCode::Enter => app.trading.show_order_detail = false,
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
        _ if app.active_tab == TRADING_TAB => handle_trading_key(app, key),
        _ if app.active_tab == EVENTS_TAB_INDEX => handle_events_key(app, key),
        _ if app.active_tab == TAB_RESEARCH => handle_research_key(app, key),
        _ if app.active_tab == STRATEGIES_TAB => handle_strategies_key(app, key),
        _ => {}
    }
}

/// Handle key presses specific to the Trading tab.
fn handle_trading_key(app: &mut App, key: KeyCode) {
    match key {
        KeyCode::Char('j') | KeyCode::Down => app.trading.select_next_order(),
        KeyCode::Char('k') | KeyCode::Up => app.trading.select_prev_order(),
        KeyCode::Enter => app.trading.toggle_order_detail(),
        KeyCode::Char('p') => app.trading.cycle_pnl_range(),
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
const fn handle_research_key(app: &mut App, key: KeyCode) {
    match key {
        KeyCode::Char('j') | KeyCode::Down => app.research.select_next(),
        KeyCode::Char('k') | KeyCode::Up => app.research.select_prev(),
        KeyCode::Char('p') => app.research.toggle_dag(),
        _ => {}
    }
}

/// Handle key presses specific to the Strategies tab.
fn handle_strategies_key(app: &mut App, key: KeyCode) {
    match key {
        KeyCode::Char('j') | KeyCode::Down => app.strategies_state.select_next(),
        KeyCode::Char('k') | KeyCode::Up => app.strategies_state.select_previous(),
        KeyCode::Enter => app.strategies_state.toggle_detail(),
        _ => {}
    }
}

/// Poll the gRPC server for system status, handling reconnection on failure.
async fn poll_status(app: &mut App, client: &mut Option<RaraServiceClient<Channel>>) -> Result<()> {
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
