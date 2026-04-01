//! Overview tab — four-quadrant cockpit view.
//!
//! Answers in 3 seconds: "What is the system doing? Is everything OK?"
//!
//! **Wide layout** (≥120 cols): dual-column (60%/40%)
//! - Left:  Strategies, Positions, Recent Events
//! - Right: System Status, Alerts, Research Progress
//!
//! **Narrow layout** (<120 cols): single-column stacked

use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::Modifier,
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Gauge, List, ListItem, Paragraph, Row, Table},
};

use crate::{app::App, theme};

/// Minimum width (in columns) to use the dual-column layout.
const WIDE_THRESHOLD: u16 = 120;

/// Render the overview tab content into the given area.
pub fn render(frame: &mut Frame, app: &App, area: Rect) {
    if area.width >= WIDE_THRESHOLD {
        render_wide(frame, app, area);
    } else {
        render_narrow(frame, app, area);
    }
}

/// Dual-column layout for wide terminals (≥120 cols).
fn render_wide(frame: &mut Frame, app: &App, area: Rect) {
    let columns =
        Layout::horizontal([Constraint::Percentage(60), Constraint::Percentage(40)]).split(area);

    // Left column: strategies, positions, recent events
    let left_panes = Layout::vertical([
        Constraint::Percentage(35),
        Constraint::Percentage(35),
        Constraint::Percentage(30),
    ])
    .split(columns[0]);

    render_strategies(frame, app, left_panes[0]);
    render_positions(frame, app, left_panes[1]);
    render_recent_events(frame, app, left_panes[2]);

    // Right column: system status, alerts, research progress
    let right_panes = Layout::vertical([
        Constraint::Percentage(30),
        Constraint::Percentage(35),
        Constraint::Percentage(35),
    ])
    .split(columns[1]);

    render_system_status(frame, app, right_panes[0]);
    render_alerts(frame, app, right_panes[1]);
    render_research_progress(frame, app, right_panes[2]);
}

/// Single-column layout for narrow terminals (<120 cols).
///
/// Renders a compact system+research status bar at top, then stacks
/// strategies, positions, alerts, and recent events vertically.
fn render_narrow(frame: &mut Frame, app: &App, area: Rect) {
    let panes = Layout::vertical([
        Constraint::Length(3), // system + research compact bar
        Constraint::Length(5), // strategies (compact)
        Constraint::Length(5), // positions (compact)
        Constraint::Length(4), // alerts
        Constraint::Min(3),    // recent events (fill)
    ])
    .split(area);

    render_narrow_status_bar(frame, app, panes[0]);
    render_strategies(frame, app, panes[1]);
    render_positions(frame, app, panes[2]);
    render_alerts(frame, app, panes[3]);
    render_recent_events(frame, app, panes[4]);
}

/// Compact single-row status bar combining system health and research progress.
fn render_narrow_status_bar(frame: &mut Frame, app: &App, area: Rect) {
    let block = pane_block(" System ");

    let mut spans: Vec<Span> = vec![Span::styled("  ", theme::muted())];

    // System connection indicators
    if let Some(status) = &app.system_status {
        spans.push(Span::styled("DB ", theme::muted()));
        spans.push(indicator_dot(status.database_connected));
        spans.push(Span::styled("  WS ", theme::muted()));
        spans.push(indicator_dot(status.websocket_connected));
        spans.push(Span::styled("  LLM ", theme::muted()));
        spans.push(indicator_dot(status.llm_available));
    } else {
        spans.push(Span::styled("Connecting...", theme::muted()));
    }

    // Research summary on the same line
    if let Some(rp) = &app.research_progress {
        spans.push(Span::styled("  | Research ", theme::muted()));
        spans.push(Span::styled(
            format!("{}/{}", rp.current, rp.total),
            ratatui::style::Style::default().fg(theme::IRIS),
        ));
        spans.push(Span::styled(
            format!(" \u{2713}{}", rp.accepted),
            theme::positive(),
        ));
        spans.push(Span::styled(
            format!(" \u{2717}{}", rp.rejected),
            theme::negative(),
        ));
        spans.push(Span::styled(
            format!(" \u{2699}{}", rp.in_progress),
            theme::warning(),
        ));
    }

    let paragraph = Paragraph::new(Line::from(spans)).block(block);
    frame.render_widget(paragraph, area);
}

/// Render the strategies table pane.
fn render_strategies(frame: &mut Frame, app: &App, area: Rect) {
    let block = pane_block(" Strategies ");

    if app.strategies.is_empty() {
        let empty = Paragraph::new("  No strategies yet. Run \"rara research run\" to start.")
            .style(theme::muted())
            .block(block);
        frame.render_widget(empty, area);
        return;
    }

    let header = Row::new(vec![
        Cell::from("Name").style(theme::emphasis()),
        Cell::from("Status").style(theme::emphasis()),
        Cell::from("PnL").style(theme::emphasis()),
        Cell::from("Sharpe").style(theme::emphasis()),
    ])
    .height(1);

    let rows: Vec<Row> = app
        .strategies
        .iter()
        .map(|s| {
            let pnl_style = pnl_style(s.pnl);
            Row::new(vec![
                Cell::from(s.name.as_str()).style(theme::text()),
                Cell::from(format!("\u{25cf}{}", s.status)).style(status_style(&s.status)),
                Cell::from(format!("{:+.2}", s.pnl)).style(pnl_style),
                Cell::from(format!("{:.2}", s.sharpe))
                    .style(ratatui::style::Style::default().fg(theme::IRIS)),
            ])
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Percentage(30),
            Constraint::Percentage(25),
            Constraint::Percentage(25),
            Constraint::Percentage(20),
        ],
    )
    .header(header)
    .block(block);

    frame.render_widget(table, area);
}

/// Render the positions table pane.
fn render_positions(frame: &mut Frame, app: &App, area: Rect) {
    let block = pane_block(" Positions ");

    if app.positions.is_empty() {
        let empty = Paragraph::new("  No open positions.")
            .style(theme::muted())
            .block(block);
        frame.render_widget(empty, area);
        return;
    }

    let header = Row::new(vec![
        Cell::from("Symbol").style(theme::emphasis()),
        Cell::from("Side").style(theme::emphasis()),
        Cell::from("Qty").style(theme::emphasis()),
        Cell::from("Entry").style(theme::emphasis()),
        Cell::from("Current").style(theme::emphasis()),
        Cell::from("PnL").style(theme::emphasis()),
    ])
    .height(1);

    let rows: Vec<Row> = app
        .positions
        .iter()
        .map(|p| {
            let pnl_style = pnl_style(p.pnl);
            let side_style = if p.side == "Long" {
                theme::positive()
            } else {
                theme::negative()
            };
            Row::new(vec![
                Cell::from(p.symbol.as_str()).style(theme::text()),
                Cell::from(p.side.as_str()).style(side_style),
                Cell::from(format!("{:.4}", p.quantity)).style(theme::text()),
                Cell::from(format!("{:.2}", p.entry_price)).style(theme::muted()),
                Cell::from(format!("{:.2}", p.current_price)).style(theme::text()),
                Cell::from(format!("{:+.2}", p.pnl)).style(pnl_style),
            ])
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Percentage(20),
            Constraint::Percentage(12),
            Constraint::Percentage(15),
            Constraint::Percentage(18),
            Constraint::Percentage(18),
            Constraint::Percentage(17),
        ],
    )
    .header(header)
    .block(block);

    frame.render_widget(table, area);
}

/// Render the recent events list pane.
fn render_recent_events(frame: &mut Frame, app: &App, area: Rect) {
    let block = pane_block(" Recent Events ");

    if app.recent_events.is_empty() {
        let empty = Paragraph::new("  Waiting for events...")
            .style(theme::muted())
            .block(block);
        frame.render_widget(empty, area);
        return;
    }

    let items: Vec<ListItem> = app
        .recent_events
        .iter()
        .map(|e| {
            let type_style = event_type_style(&e.event_type);
            ListItem::new(Line::from(vec![
                Span::styled(&e.time, theme::muted()),
                Span::styled(" [", theme::muted()),
                Span::styled(&e.event_type, type_style),
                Span::styled("] ", theme::muted()),
                Span::styled(&e.summary, theme::text()),
            ]))
        })
        .collect();

    let list = List::new(items).block(block);
    frame.render_widget(list, area);
}

/// Render the system status summary pane.
fn render_system_status(frame: &mut Frame, app: &App, area: Rect) {
    let block = pane_block(" System Status ");

    let lines: Vec<Line> = app.system_status.as_ref().map_or_else(
        || {
            vec![Line::from(Span::styled(
                "  Connecting to server...",
                theme::muted(),
            ))]
        },
        |status| {
            let mut lines = vec![
                Line::from(vec![
                    Span::styled("  DB: ", theme::muted()),
                    connection_span(status.database_connected),
                    Span::styled("   WS: ", theme::muted()),
                    connection_span(status.websocket_connected),
                    Span::styled("   LLM: ", theme::muted()),
                    connection_span(status.llm_available),
                ]),
                Line::from(vec![
                    Span::styled("  Strategies: ", theme::muted()),
                    Span::styled(format!("{}", status.strategy_count), theme::text()),
                    Span::styled("   Contracts: ", theme::muted()),
                    Span::styled(format!("{}", status.contract_count), theme::text()),
                    Span::styled("   Uptime: ", theme::muted()),
                    Span::styled(&status.uptime, theme::text()),
                ]),
            ];
            // Show actionable warnings when services are misconfigured
            for warning in &status.warnings {
                lines.push(Line::from(Span::styled(
                    format!("  ! {warning}"),
                    theme::warning(),
                )));
            }
            lines
        },
    );

    let paragraph = Paragraph::new(lines).block(block);
    frame.render_widget(paragraph, area);
}

/// Render the alerts pane.
fn render_alerts(frame: &mut Frame, app: &App, area: Rect) {
    let block = pane_block(" Alerts ");

    if app.alerts.is_empty() {
        let empty = Paragraph::new("  No alerts. All clear.")
            .style(theme::muted())
            .block(block);
        frame.render_widget(empty, area);
        return;
    }

    let items: Vec<ListItem> = app
        .alerts
        .iter()
        .map(|alert| {
            ListItem::new(Line::from(vec![
                Span::styled("  ⚠ ", theme::warning()),
                Span::styled(alert.as_str(), theme::warning()),
            ]))
        })
        .collect();

    let list = List::new(items).block(block);
    frame.render_widget(list, area);
}

/// Render the research progress pane with a progress bar.
fn render_research_progress(frame: &mut Frame, app: &App, area: Rect) {
    let block = pane_block(" Research Progress ");

    if let Some(rp) = &app.research_progress {
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let panes = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .split(inner);

        // Summary line
        let summary = Line::from(vec![
            Span::styled("  Accepted: ", theme::muted()),
            Span::styled(format!("{}", rp.accepted), theme::positive()),
            Span::styled("  Rejected: ", theme::muted()),
            Span::styled(format!("{}", rp.rejected), theme::negative()),
            Span::styled("  In-progress: ", theme::muted()),
            Span::styled(format!("{}", rp.in_progress), theme::warning()),
            rp.sota_sharpe.map_or_else(
                || Span::raw(""),
                |sharpe| Span::styled(format!("  SOTA Sharpe: {sharpe:.2}"), theme::positive()),
            ),
        ]);
        frame.render_widget(Paragraph::new(summary), panes[0]);

        // Progress gauge
        let ratio = if rp.total > 0 {
            f64::from(rp.current) / f64::from(rp.total)
        } else {
            0.0
        };
        let gauge = Gauge::default()
            .gauge_style(
                ratatui::style::Style::default()
                    .fg(theme::IRIS)
                    .bg(theme::OVERLAY)
                    .add_modifier(Modifier::BOLD),
            )
            .ratio(ratio.clamp(0.0, 1.0))
            .label(format!("{}/{}", rp.current, rp.total));
        frame.render_widget(gauge, panes[1]);
    } else {
        let empty = Paragraph::new("  Research loop not running.")
            .style(theme::muted())
            .block(block);
        frame.render_widget(empty, area);
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Create a styled pane block with borders and title.
fn pane_block(title: &str) -> Block<'_> {
    Block::default()
        .title(title)
        .title_style(theme::emphasis())
        .borders(Borders::ALL)
        .border_style(ratatui::style::Style::default().fg(theme::OVERLAY))
        .style(ratatui::style::Style::default().bg(theme::BASE))
}

/// Return a style for `PnL` values: positive → green, negative → red, zero →
/// muted.
fn pnl_style(pnl: f64) -> ratatui::style::Style {
    if pnl > 0.0 {
        theme::positive()
    } else if pnl < 0.0 {
        theme::negative()
    } else {
        theme::muted()
    }
}

/// Return a style for strategy status strings.
fn status_style(status: &str) -> ratatui::style::Style {
    match status {
        "Running" => theme::positive(),
        "Promoted" => theme::highlight(),
        "Stopped" | "Demoted" => theme::negative(),
        "Paper" => theme::warning(),
        _ => theme::muted(),
    }
}

/// Return a style for event type tags.
fn event_type_style(event_type: &str) -> ratatui::style::Style {
    match event_type {
        "ERROR" | "ALERT" => theme::negative(),
        "WARN" => theme::warning(),
        "TRADE" | "FILL" => theme::positive(),
        "INFO" => theme::info(),
        _ => theme::muted(),
    }
}

/// Return a connection status span (connected/disconnected).
fn connection_span(connected: bool) -> Span<'static> {
    if connected {
        Span::styled("\u{25cf} Connected", theme::positive())
    } else {
        Span::styled("\u{25cf} Disconnected", theme::negative())
    }
}

/// Return a colored dot indicator for compact layouts.
fn indicator_dot(connected: bool) -> Span<'static> {
    if connected {
        Span::styled("\u{25cf}", theme::positive())
    } else {
        Span::styled("\u{25cf}", theme::negative())
    }
}
