//! Trading tab — account summary, positions, orders, and `PnL` sparkline.
//!
//! Layout (vertical stack):
//! 1. Account bar (1 line): Equity, Cash, Unrealized `PnL`, Day change%
//! 2. Positions table: Symbol, Side, Qty, Entry, Current, `PnL`, Strategy
//! 3. Orders table: Time, Symbol, Side, Qty, Price, Status, Strategy, Guard
//! 4. `PnL` sparkline (4 lines): ratatui `Sparkline` widget

use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Sparkline, Table};
use ratatui::Frame;

use crate::app::{App, PnlRange, TradingState};
use crate::theme;

/// Render the full trading tab content into the given area.
pub fn render(frame: &mut Frame, app: &App, area: Rect) {
    let trading = &app.trading;

    let chunks = Layout::vertical([
        Constraint::Length(1),  // account bar
        Constraint::Min(5),    // positions table
        Constraint::Min(8),    // orders table
        Constraint::Length(6), // PnL sparkline
    ])
    .split(area);

    render_account_bar(frame, trading, chunks[0]);
    render_positions_table(frame, trading, chunks[1]);
    render_orders_table(frame, trading, chunks[2]);
    render_pnl_sparkline(frame, trading, chunks[3]);
}

/// Render the single-line account summary bar.
fn render_account_bar(frame: &mut Frame, state: &TradingState, area: Rect) {
    let acct = &state.account;
    let unrealized_style = pnl_style(acct.unrealized_pnl);
    let day_style = pnl_style(acct.day_change_pct);

    let line = Line::from(vec![
        Span::styled(" Equity: ", theme::muted()),
        Span::styled(format!("{:.2}", acct.equity), theme::text()),
        Span::styled("  Cash: ", theme::muted()),
        Span::styled(format!("{:.2}", acct.cash), theme::text()),
        Span::styled("  Unrealized PnL: ", theme::muted()),
        Span::styled(format!("{:+.2}", acct.unrealized_pnl), unrealized_style),
        Span::styled("  Day: ", theme::muted()),
        Span::styled(format!("{:+.2}%", acct.day_change_pct), day_style),
    ]);

    let bar = Paragraph::new(line).style(theme::status_bar_bg());
    frame.render_widget(bar, area);
}

/// Render the open positions table, or a centered "No open positions." message.
fn render_positions_table(frame: &mut Frame, state: &TradingState, area: Rect) {
    let block = Block::default()
        .title(" Positions ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme::OVERLAY));

    if state.positions.is_empty() {
        let empty = Paragraph::new("No open positions.")
            .alignment(Alignment::Center)
            .style(theme::muted())
            .block(block);
        frame.render_widget(empty, area);
        return;
    }

    let header = Row::new(["Symbol", "Side", "Qty", "Entry", "Current", "PnL", "Strategy"])
        .style(theme::emphasis())
        .bottom_margin(0);

    let rows = state.positions.iter().map(|p| {
        let pnl_s = pnl_style(p.pnl);
        Row::new([
            Cell::from(p.symbol.as_str()).style(theme::text()),
            Cell::from(p.side.as_str()).style(side_style(&p.side)),
            Cell::from(format!("{:.4}", p.quantity)).style(theme::text()),
            Cell::from(format!("{:.2}", p.entry_price)).style(theme::text()),
            Cell::from(format!("{:.2}", p.current_price)).style(theme::text()),
            Cell::from(format!("{:+.2}", p.pnl)).style(pnl_s),
            Cell::from(p.strategy.as_str()).style(theme::info()),
        ])
    });

    let widths = [
        Constraint::Length(12),
        Constraint::Length(6),
        Constraint::Length(10),
        Constraint::Length(10),
        Constraint::Length(10),
        Constraint::Length(12),
        Constraint::Fill(1),
    ];

    let table = Table::new(rows, widths).header(header).block(block);
    frame.render_widget(table, area);
}

/// Render the orders table with status and guard indicators.
fn render_orders_table(frame: &mut Frame, state: &TradingState, area: Rect) {
    let block = Block::default()
        .title(" Orders ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme::OVERLAY));

    let header = Row::new([
        "Time", "Symbol", "Side", "Qty", "Price", "Status", "Strategy", "Guard",
    ])
    .style(theme::emphasis())
    .bottom_margin(0);

    let rows = state.orders.iter().enumerate().map(|(i, o)| {
        let is_selected = i == state.selected_order;
        let base = if is_selected {
            Style::default().bg(theme::OVERLAY)
        } else {
            Style::default()
        };

        Row::new([
            Cell::from(o.time.as_str()).style(base.fg(theme::MUTED)),
            Cell::from(o.symbol.as_str()).style(base.fg(theme::TEXT)),
            Cell::from(o.side.as_str()).style(base.patch(side_style(&o.side))),
            Cell::from(format!("{:.4}", o.quantity)).style(base.fg(theme::TEXT)),
            Cell::from(format!("{:.2}", o.price)).style(base.fg(theme::TEXT)),
            Cell::from(status_display(&o.status)).style(base.patch(status_style(&o.status))),
            Cell::from(o.strategy.as_str()).style(base.fg(theme::PINE)),
            Cell::from(guard_display(&o.guard_result))
                .style(base.patch(guard_style(&o.guard_result))),
        ])
    });

    let widths = [
        Constraint::Length(8),
        Constraint::Length(12),
        Constraint::Length(6),
        Constraint::Length(10),
        Constraint::Length(10),
        Constraint::Length(10),
        Constraint::Length(12),
        Constraint::Fill(1),
    ];

    let table = Table::new(rows, widths).header(header).block(block);
    frame.render_widget(table, area);
}

/// Render the `PnL` sparkline chart.
fn render_pnl_sparkline(frame: &mut Frame, state: &TradingState, area: Rect) {
    let range_label = match state.pnl_range {
        PnlRange::Hour1 => "1h",
        PnlRange::Hour4 => "4h",
        PnlRange::Day1 => "1d",
        PnlRange::All => "all",
    };

    let block = Block::default()
        .title(format!(" PnL ({range_label}) "))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme::OVERLAY));

    let sparkline = Sparkline::default()
        .block(block)
        .data(&state.pnl_data)
        .style(Style::default().fg(theme::FOAM));

    frame.render_widget(sparkline, area);
}

// ---------------------------------------------------------------------------
// Style helpers
// ---------------------------------------------------------------------------

/// Return FOAM for positive values, LOVE for negative, MUTED for zero.
fn pnl_style(value: f64) -> Style {
    if value > 0.0 {
        theme::positive()
    } else if value < 0.0 {
        theme::negative()
    } else {
        theme::muted()
    }
}

/// Color the side indicator: Buy=FOAM, Sell=LOVE.
fn side_style(side: &str) -> Style {
    match side.to_uppercase().as_str() {
        "BUY" | "LONG" => Style::default().fg(theme::FOAM),
        "SELL" | "SHORT" => Style::default().fg(theme::LOVE),
        _ => theme::text(),
    }
}

/// Format order status with icon prefix.
fn status_display(status: &str) -> String {
    match status.to_uppercase().as_str() {
        "FILLED" => "\u{2713}Filled".to_string(),
        "REJECTED" => "\u{2717}Reject".to_string(),
        "SUBMITTED" => "\u{23f3}Submit".to_string(),
        _ => status.to_string(),
    }
}

/// Style for order status.
fn status_style(status: &str) -> Style {
    match status.to_uppercase().as_str() {
        "FILLED" => Style::default().fg(theme::FOAM),
        "REJECTED" => Style::default().fg(theme::LOVE),
        "SUBMITTED" => Style::default().fg(theme::GOLD),
        _ => theme::muted(),
    }
}

/// Format guard result with icon prefix.
fn guard_display(guard: &str) -> String {
    if guard.is_empty() || guard.eq_ignore_ascii_case("pass") {
        "\u{2713}Pass".to_string()
    } else {
        format!("\u{2717}{guard}")
    }
}

/// Style for guard result.
fn guard_style(guard: &str) -> Style {
    if guard.is_empty() || guard.eq_ignore_ascii_case("pass") {
        Style::default().fg(theme::FOAM)
    } else {
        Style::default().fg(theme::LOVE)
    }
}

/// Render the order detail overlay when `show_order_detail` is true.
pub fn render_order_detail_overlay(frame: &mut Frame, app: &App) {
    let trading = &app.trading;
    let Some(order) = trading.orders.get(trading.selected_order) else {
        return;
    };

    let area = frame.area();
    // Center a popup that is 60 cols x 12 rows
    let popup_width = 60u16.min(area.width.saturating_sub(4));
    let popup_height = 12u16.min(area.height.saturating_sub(4));
    let x = (area.width.saturating_sub(popup_width)) / 2;
    let y = (area.height.saturating_sub(popup_height)) / 2;
    let popup_area = Rect::new(x, y, popup_width, popup_height);

    let block = Block::default()
        .title(" Order Detail ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme::IRIS))
        .style(Style::default().bg(theme::SURFACE));

    let lines = vec![
        Line::from(vec![
            Span::styled("Time:     ", theme::muted()),
            Span::styled(&order.time, theme::text()),
        ]),
        Line::from(vec![
            Span::styled("Symbol:   ", theme::muted()),
            Span::styled(&order.symbol, theme::text()),
        ]),
        Line::from(vec![
            Span::styled("Side:     ", theme::muted()),
            Span::styled(&order.side, side_style(&order.side)),
        ]),
        Line::from(vec![
            Span::styled("Quantity: ", theme::muted()),
            Span::styled(format!("{:.4}", order.quantity), theme::text()),
        ]),
        Line::from(vec![
            Span::styled("Price:    ", theme::muted()),
            Span::styled(format!("{:.2}", order.price), theme::text()),
        ]),
        Line::from(vec![
            Span::styled("Status:   ", theme::muted()),
            Span::styled(status_display(&order.status), status_style(&order.status)),
        ]),
        Line::from(vec![
            Span::styled("Strategy: ", theme::muted()),
            Span::styled(&order.strategy, theme::info()),
        ]),
        Line::from(vec![
            Span::styled("Guard:    ", theme::muted()),
            Span::styled(
                guard_display(&order.guard_result),
                guard_style(&order.guard_result),
            ),
        ]),
        Line::default(),
        Line::from(Span::styled(
            "Press Esc or Enter to close",
            theme::muted().add_modifier(Modifier::ITALIC),
        )),
    ];

    let detail = Paragraph::new(lines).block(block);
    // Clear background area first
    let clear = Block::default().style(Style::default().bg(theme::SURFACE));
    frame.render_widget(clear, popup_area);
    frame.render_widget(detail, popup_area);
}
