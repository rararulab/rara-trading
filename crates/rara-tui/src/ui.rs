//! Rendering logic for the TUI layout.
//!
//! Layout:
//! ```text
//! ┌─ Status Bar ──────────────────────────────────────────────┐
//! │ ● DB  ● WS  ● LLM │ 0 strategies │ localhost:50051 │00:00│
//! ├─ Tabs ────────────────────────────────────────────────────┤
//! │ [1] Overview  [2] Research  [3] Trading  [4] Strategies   │
//! ├───────────────────────────────────────────────────────────┤
//! │                    Tab Content Area                        │
//! │                    (placeholder for now)                   │
//! ├─ Footer ──────────────────────────────────────────────────┤
//! │ q:Quit  1-4:Tab  ?:Help                                   │
//! └───────────────────────────────────────────────────────────┘
//! ```

use ratatui::layout::{Constraint, Layout};
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Tabs};
use ratatui::Frame;

use crate::app::{App, ConnectionStatus, EVENTS_TAB_INDEX, STRATEGIES_TAB, TAB_NAMES, TAB_RESEARCH, TRADING_TAB};
use crate::tabs;
use crate::tabs::research;
use crate::tabs::strategies as strategies_tab;
use crate::tabs::trading;
use crate::theme;

/// Render the full dashboard to the terminal frame.
pub fn render(frame: &mut Frame, app: &App) {
    let area = frame.area();

    // Global layout: status bar (1) + tabs (1) + content (fill) + footer (1)
    let chunks = Layout::vertical([
        Constraint::Length(1), // status bar
        Constraint::Length(2), // tab bar with border
        Constraint::Min(3),    // content area
        Constraint::Length(1), // footer
    ])
    .split(area);

    render_status_bar(frame, app, chunks[0]);
    render_tab_bar(frame, app, chunks[1]);
    render_content(frame, app, chunks[2]);
    render_footer(frame, app, chunks[3]);

    // Render overlays on top of everything
    if app.active_tab == TRADING_TAB && app.trading.show_order_detail {
        trading::render_order_detail_overlay(frame, app);
    }
}

/// Render the top status bar showing connection indicators and server info.
fn render_status_bar(frame: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let (db_style, ws_style, llm_style) = app.system_status.as_ref().map_or_else(
        || (theme::muted(), theme::muted(), theme::muted()),
        |status| {
            let style_for = |connected| {
                if connected {
                    theme::positive()
                } else {
                    theme::negative()
                }
            };
            (
                style_for(status.database_connected),
                style_for(status.websocket_connected),
                style_for(status.llm_available),
            )
        },
    );

    let strategy_count = app
        .system_status
        .as_ref()
        .map_or(0, |s| s.strategy_count);
    let uptime = app
        .system_status
        .as_ref()
        .map_or_else(|| "--:--:--".to_string(), |s| s.uptime.clone());

    let connection_indicator = match &app.connection_status {
        ConnectionStatus::Connected => Span::styled("● ", theme::positive()),
        ConnectionStatus::Connecting => Span::styled("◌ ", theme::warning()),
        ConnectionStatus::Disconnected { .. } => Span::styled("● ", theme::negative()),
    };

    let line = Line::from(vec![
        Span::styled(" ", theme::status_bar_bg()),
        connection_indicator,
        Span::styled("DB ", db_style),
        Span::styled("● ", db_style),
        Span::styled("WS ", ws_style),
        Span::styled("● ", ws_style),
        Span::styled("LLM", llm_style),
        Span::styled(" │ ", theme::muted()),
        Span::styled(format!("{strategy_count} strategies"), theme::text()),
        Span::styled(" │ ", theme::muted()),
        Span::styled(&app.server_addr, theme::info()),
        Span::styled(" │ ", theme::muted()),
        Span::styled(uptime, theme::muted()),
    ]);

    let bar = Paragraph::new(line).style(theme::status_bar_bg());
    frame.render_widget(bar, area);
}

/// Render the tab navigation bar.
fn render_tab_bar(frame: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let titles: Vec<Line> = TAB_NAMES
        .iter()
        .enumerate()
        .map(|(i, name)| {
            let num = i + 1;
            Line::from(format!(" {num} {name} "))
        })
        .collect();

    let tabs = Tabs::new(titles)
        .block(Block::default().borders(Borders::BOTTOM))
        .select(app.active_tab)
        .style(theme::muted())
        .highlight_style(
            theme::emphasis()
                .add_modifier(Modifier::UNDERLINED)
                .fg(theme::ROSE),
        )
        .divider("│");

    frame.render_widget(tabs, area);
}

/// Render the main content area, dispatching to tab-specific renderers.
fn render_content(frame: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    // Show connection overlay when not connected, regardless of tab
    match &app.connection_status {
        ConnectionStatus::Connecting => {
            let content = Paragraph::new(format!("Connecting to {}...", app.server_addr))
                .style(theme::warning())
                .block(content_block());
            frame.render_widget(content, area);
            return;
        }
        ConnectionStatus::Disconnected { retry_count } => {
            let content = Paragraph::new(format!(
                "Connection lost. Reconnecting... (attempt {retry_count})\n\
                 Server: {}",
                app.server_addr
            ))
            .style(theme::negative())
            .block(content_block());
            frame.render_widget(content, area);
            return;
        }
        ConnectionStatus::Connected => {}
    }

    // Dispatch to tab-specific renderer
    if app.active_tab == 0 {
        tabs::overview::render(frame, app, area);
    } else if app.active_tab == TAB_RESEARCH {
        research::render(frame, &app.research, area);
    } else if app.active_tab == TRADING_TAB {
        trading::render(frame, app, area);
    } else if app.active_tab == STRATEGIES_TAB {
        strategies_tab::render(frame, &app.strategies_state, area);
    } else if app.active_tab == EVENTS_TAB_INDEX {
        tabs::events::render(frame, &app.events_state, area);
    } else {
        let tab_name = TAB_NAMES
            .get(app.active_tab)
            .copied()
            .unwrap_or("Unknown");
        let content = Paragraph::new(format!(
            "{tab_name}\n\nContent will be implemented in future issues."
        ))
        .style(theme::text())
        .block(content_block());
        frame.render_widget(content, area);
    }
}

/// Shared content area block style.
fn content_block() -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_style(theme::muted().fg(theme::OVERLAY))
        .style(ratatui::style::Style::default().bg(theme::BASE))
}

/// Render the footer with keyboard shortcuts.
fn render_footer(frame: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let footer_text = match &app.connection_status {
        ConnectionStatus::Disconnected { retry_count } => {
            format!("Connection lost. Reconnecting... (attempt {retry_count})  │  q:Quit")
        }
        _ if app.active_tab == TAB_RESEARCH => {
            "q:Quit  1-5:Tab  j/k:Navigate  p:DAG  ?:Help".to_string()
        }
        _ if app.active_tab == TRADING_TAB => {
            "q:Quit  1-5:Tab  j/k:Navigate  Enter:Detail  p:PnL range  ?:Help".to_string()
        }
        _ if app.active_tab == STRATEGIES_TAB => {
            "q:Quit  1-5:Tab  j/k:Navigate  Enter:Detail  d:DAG  ?:Help".to_string()
        }
        _ if app.active_tab == EVENTS_TAB_INDEX => {
            if app.events_state.search_active {
                "Esc:Cancel  Enter:Confirm search".to_string()
            } else {
                "q:Quit  1-5:Tab  Space:Pause  j/k:Nav  /:Search  G:Latest  Enter:Detail"
                    .to_string()
            }
        }
        _ => "q:Quit  1-5:Tab  ?:Help".to_string(),
    };

    let footer = Paragraph::new(footer_text).style(theme::footer());
    frame.render_widget(footer, area);
}
