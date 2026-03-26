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

use crate::app::{App, ConnectionStatus, TAB_NAMES};
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

/// Render the main content area (placeholder per tab).
fn render_content(frame: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let content_text = match &app.connection_status {
        ConnectionStatus::Connecting => {
            format!("Connecting to {}...", app.server_addr)
        }
        ConnectionStatus::Disconnected { retry_count } => {
            format!(
                "Connection lost. Reconnecting... (attempt {retry_count})\n\
                 Server: {}",
                app.server_addr
            )
        }
        ConnectionStatus::Connected => {
            let tab_name = TAB_NAMES
                .get(app.active_tab)
                .copied()
                .unwrap_or("Unknown");
            format!("{tab_name}\n\nContent will be implemented in future issues.")
        }
    };

    let style = match &app.connection_status {
        ConnectionStatus::Connected => theme::text(),
        ConnectionStatus::Connecting => theme::warning(),
        ConnectionStatus::Disconnected { .. } => theme::negative(),
    };

    let content = Paragraph::new(content_text)
        .style(style)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(theme::muted().fg(theme::OVERLAY))
                .style(ratatui::style::Style::default().bg(theme::BASE)),
        );

    frame.render_widget(content, area);
}

/// Render the footer with keyboard shortcuts.
fn render_footer(frame: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let footer_text = match &app.connection_status {
        ConnectionStatus::Disconnected { retry_count } => {
            format!("Connection lost. Reconnecting... (attempt {retry_count})  │  q:Quit")
        }
        _ => "q:Quit  1-4:Tab  ?:Help".to_string(),
    };

    let footer = Paragraph::new(footer_text).style(theme::footer());
    frame.render_widget(footer, area);
}
