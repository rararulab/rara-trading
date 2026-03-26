//! Strategies tab — displays real installed strategies from the registry.
//!
//! Layout:
//! ```text
//! ┌─ Installed Strategies ─────────────────────────────────────────┐
//! │ Name       Ver  API  Release    Size    Status                  │
//! │ > btc-mom   1   v1   v0.1.0    128KB   ● Installed             │
//! │   hmm-reg   2   v1   v0.2.0    256KB   ● Installed             │
//! ├─ Detail ───────────────────────────────────────────────────────┤
//! │ Tag: btc-momentum-v0.1.0                                       │
//! │ Description: "BTC momentum breakout strategy"                  │
//! │ WASM: ~/.rara-trading/strategies/promoted/btc-momentum.wasm    │
//! │ Download URL: https://github.com/...                           │
//! └───────────────────────────────────────────────────────────────┘
//! ```

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table};
use ratatui::Frame;

use crate::app::{StrategiesState, StrategyLifecycle};
use crate::theme;

/// Render the full strategies tab into the given area.
pub fn render(frame: &mut Frame, state: &StrategiesState, area: Rect) {
    if state.strategies.is_empty() {
        render_empty(frame, area);
        return;
    }

    let chunks = Layout::vertical([
        Constraint::Percentage(55), // strategy list
        Constraint::Percentage(45), // detail panel
    ])
    .split(area);

    render_strategy_list(frame, state, chunks[0]);
    render_detail(frame, state, chunks[1]);
}

/// Render a placeholder message when no strategies are installed.
fn render_empty(frame: &mut Frame, area: Rect) {
    let content = vec![
        Line::from(""),
        Line::from(Span::styled(
            "No strategies installed",
            Style::default()
                .fg(theme::MUTED)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "Use `rara-trading strategies fetch <name>` to install strategies from the registry.",
            theme::muted(),
        )),
    ];

    let block = Paragraph::new(content)
        .block(
            Block::default()
                .title(" Strategies ")
                .borders(Borders::ALL)
                .border_style(theme::muted()),
        )
        .alignment(ratatui::layout::Alignment::Center);

    frame.render_widget(block, area);
}

/// Render the strategy list table with status indicators.
fn render_strategy_list(frame: &mut Frame, state: &StrategiesState, area: Rect) {
    let header = Row::new(vec![
        Cell::from("Name").style(theme::emphasis()),
        Cell::from("Ver").style(theme::emphasis()),
        Cell::from("API").style(theme::emphasis()),
        Cell::from("Release").style(theme::emphasis()),
        Cell::from("Size").style(theme::emphasis()),
        Cell::from("Status").style(theme::emphasis()),
    ])
    .height(1);

    let rows: Vec<Row> = state
        .strategies
        .iter()
        .enumerate()
        .map(|(i, entry)| {
            let is_selected = i == state.selected_index;
            let base_style = if is_selected {
                Style::default().fg(theme::TEXT).bg(theme::SURFACE)
            } else {
                theme::text()
            };

            let (status_icon, status_label, status_style) = lifecycle_style(&entry.status);

            Row::new(vec![
                Cell::from(entry.name.clone()).style(base_style),
                Cell::from(format!("v{}", entry.version)).style(base_style),
                Cell::from(format!("v{}", entry.api_version)).style(base_style),
                Cell::from(entry.release_version.clone()).style(base_style),
                Cell::from(format_file_size(entry.file_size)).style(base_style),
                Cell::from(Line::from(vec![
                    Span::styled(status_icon, status_style),
                    Span::styled(format!(" {status_label}"), status_style),
                ])),
            ])
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Percentage(25),
            Constraint::Length(5),
            Constraint::Length(5),
            Constraint::Percentage(15),
            Constraint::Length(9),
            Constraint::Percentage(15),
        ],
    )
    .header(header)
    .block(
        Block::default()
            .title(" Installed Strategies ")
            .borders(Borders::ALL)
            .border_style(theme::muted()),
    )
    .row_highlight_style(Style::default().bg(theme::OVERLAY));

    frame.render_widget(table, area);
}

/// Render the detail panel for the currently selected strategy.
fn render_detail(frame: &mut Frame, state: &StrategiesState, area: Rect) {
    let content = state.strategies.get(state.selected_index).map_or_else(
        || {
            vec![Line::from(Span::styled(
                "No strategy selected",
                theme::muted(),
            ))]
        },
        |entry| {
            let description_text = if state.show_detail {
                entry.description.clone()
            } else {
                let truncated: String = entry.description.chars().take(80).collect();
                if entry.description.len() > 80 {
                    format!("{truncated}... [Enter to expand]")
                } else {
                    truncated
                }
            };

            vec![
                Line::from(vec![
                    Span::styled("Tag: ", theme::muted()),
                    Span::styled(entry.tag.clone(), theme::text()),
                ]),
                Line::from(vec![
                    Span::styled("Description: ", theme::muted()),
                    Span::styled(format!("\"{description_text}\""), theme::highlight()),
                ]),
                Line::from(vec![
                    Span::styled("WASM: ", theme::muted()),
                    Span::styled(entry.wasm_path.display().to_string(), theme::text()),
                ]),
                Line::from(vec![
                    Span::styled("Size: ", theme::muted()),
                    Span::styled(format_file_size(entry.file_size), theme::text()),
                    Span::styled("  API version: ", theme::muted()),
                    Span::styled(format!("v{}", entry.api_version), theme::text()),
                    Span::styled("  Strategy version: ", theme::muted()),
                    Span::styled(format!("v{}", entry.version), theme::text()),
                ]),
                Line::from(vec![
                    Span::styled("Download URL: ", theme::muted()),
                    Span::styled(entry.wasm_url.clone(), theme::muted()),
                ]),
            ]
        },
    );

    let detail = Paragraph::new(content).block(
        Block::default()
            .title(" Detail ")
            .borders(Borders::ALL)
            .border_style(theme::muted()),
    );

    frame.render_widget(detail, area);
}

/// Return the status icon, label, and style for a lifecycle status.
fn lifecycle_style(status: &StrategyLifecycle) -> (&'static str, &'static str, Style) {
    match status {
        StrategyLifecycle::Installed => (
            "\u{25cf}",
            "Installed",
            Style::default()
                .fg(theme::FOAM)
                .add_modifier(Modifier::BOLD),
        ),
    }
}

/// Format a byte count into a human-readable size string.
#[allow(clippy::cast_precision_loss)] // file sizes are well within f64 precision
fn format_file_size(bytes: u64) -> String {
    if bytes >= 1_048_576 {
        format!("{:.1} MB", bytes as f64 / 1_048_576.0)
    } else if bytes >= 1024 {
        format!("{:.0} KB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} B")
    }
}
