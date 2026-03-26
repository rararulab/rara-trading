//! Strategies tab — lifecycle master-detail view with evaluation timeline.
//!
//! Layout:
//! ```text
//! ┌─ Strategy List ──────────────────────────────────────────────┐
//! │ Strategy  Ver  Status  Sharpe  DD  Trades  Last Eval         │
//! │ > MeanRev  3   ● Active  1.42  -8%  142   2026-03-25        │
//! │   Momentum 2   ○ Demoted 0.91  -12% 89    2026-03-24        │
//! ├─ Detail ─────────────────────────────────────────────────────┤
//! │ Origin: "BTC mean reversion on 4h RSI oversold"              │
//! │ Created: 2026-01-15  Promoted: 2026-02-10                    │
//! ├─ Evaluation Timeline ────────────────────────────────────────┤
//! │ Time       Trades  Sharpe  DD    Decision  Reason            │
//! │ 2026-03-25  42     1.42   -8%   ✓Promote  Consistent alpha  │
//! │ 2026-03-18  38     1.31   -9%   —Hold     Needs more data   │
//! └─────────────────────────────────────────────────────────────┘
//! ```

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table};
use ratatui::Frame;

use crate::app::StrategiesState;
use crate::theme;

/// Render the full strategies tab into the given area.
pub fn render(frame: &mut Frame, state: &StrategiesState, area: Rect) {
    let chunks = Layout::vertical([
        Constraint::Percentage(40), // strategy list
        Constraint::Percentage(25), // detail panel
        Constraint::Percentage(35), // evaluation timeline
    ])
    .split(area);

    render_strategy_list(frame, state, chunks[0]);
    render_detail(frame, state, chunks[1]);
    render_evaluation_timeline(frame, state, chunks[2]);
}

/// Render the strategy list table with colored status indicators.
fn render_strategy_list(frame: &mut Frame, state: &StrategiesState, area: Rect) {
    let header = Row::new(vec![
        Cell::from("Strategy").style(theme::emphasis()),
        Cell::from("Ver").style(theme::emphasis()),
        Cell::from("Status").style(theme::emphasis()),
        Cell::from("Sharpe").style(theme::emphasis()),
        Cell::from("DD").style(theme::emphasis()),
        Cell::from("Trades").style(theme::emphasis()),
        Cell::from("Last Eval").style(theme::emphasis()),
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
                Cell::from(Line::from(vec![
                    Span::styled(status_icon, status_style),
                    Span::styled(format!(" {status_label}"), status_style),
                ])),
                Cell::from(format!("{:.2}", entry.sharpe)).style(base_style),
                Cell::from(format!("{:.1}%", entry.max_drawdown)).style(base_style),
                Cell::from(format!("{}", entry.trade_count)).style(base_style),
                Cell::from(
                    entry
                        .last_eval
                        .as_deref()
                        .unwrap_or("—")
                        .to_string(),
                )
                .style(theme::muted()),
            ])
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Percentage(20),
            Constraint::Length(5),
            Constraint::Percentage(14),
            Constraint::Length(7),
            Constraint::Length(7),
            Constraint::Length(7),
            Constraint::Percentage(15),
        ],
    )
    .header(header)
    .block(
        Block::default()
            .title(" Strategies ")
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
            let hypothesis_text = if state.show_detail {
                entry.origin_hypothesis.clone()
            } else {
                // Truncate to first line or 80 chars
                let truncated: String =
                    entry.origin_hypothesis.chars().take(80).collect();
                if entry.origin_hypothesis.len() > 80 {
                    format!("{truncated}... [Enter to expand]")
                } else {
                    truncated
                }
            };

            let promoted_text = entry.promoted_at.as_deref().unwrap_or("\u{2014}");

            vec![
                Line::from(vec![
                    Span::styled("Origin: ", theme::muted()),
                    Span::styled(format!("\"{hypothesis_text}\""), theme::highlight()),
                ]),
                Line::from(vec![
                    Span::styled("Created: ", theme::muted()),
                    Span::styled(entry.created_at.clone(), theme::text()),
                    Span::styled("  Promoted: ", theme::muted()),
                    Span::styled(promoted_text.to_string(), theme::text()),
                ]),
                Line::from(vec![
                    Span::styled("Sharpe: ", theme::muted()),
                    Span::styled(format!("{:.2}", entry.sharpe), theme::text()),
                    Span::styled("  Max DD: ", theme::muted()),
                    Span::styled(format!("{:.1}%", entry.max_drawdown), theme::text()),
                    Span::styled("  Trades: ", theme::muted()),
                    Span::styled(format!("{}", entry.trade_count), theme::text()),
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

/// Render the evaluation history timeline table.
fn render_evaluation_timeline(frame: &mut Frame, state: &StrategiesState, area: Rect) {
    let header = Row::new(vec![
        Cell::from("Time").style(theme::emphasis()),
        Cell::from("Trades").style(theme::emphasis()),
        Cell::from("Sharpe").style(theme::emphasis()),
        Cell::from("DD").style(theme::emphasis()),
        Cell::from("Decision").style(theme::emphasis()),
        Cell::from("Reason").style(theme::emphasis()),
    ])
    .height(1);

    let rows: Vec<Row> = state
        .evaluations
        .iter()
        .map(|eval| {
            let decision_span = decision_styled(&eval.decision);

            Row::new(vec![
                Cell::from(eval.time.clone()).style(theme::muted()),
                Cell::from(format!("{}", eval.trades)).style(theme::text()),
                Cell::from(format!("{:.2}", eval.sharpe)).style(theme::text()),
                Cell::from(format!("{:.1}%", eval.drawdown)).style(theme::text()),
                Cell::from(Line::from(decision_span)),
                Cell::from(eval.reason.clone()).style(theme::muted()),
            ])
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Percentage(15),
            Constraint::Length(7),
            Constraint::Length(7),
            Constraint::Length(7),
            Constraint::Percentage(13),
            Constraint::Percentage(30),
        ],
    )
    .header(header)
    .block(
        Block::default()
            .title(" Evaluation Timeline ")
            .borders(Borders::ALL)
            .border_style(theme::muted()),
    );

    frame.render_widget(table, area);
}

/// Return the status icon, label, and style for a lifecycle status.
fn lifecycle_style(status: &StrategyLifecycle) -> (&'static str, &'static str, Style) {
    use StrategyLifecycle::{Active, Archived, Demoted, Promoted, Retired};
    match status {
        Promoted => (
            "\u{25cf}",
            "Promoted",
            Style::default()
                .fg(theme::FOAM)
                .add_modifier(Modifier::BOLD),
        ),
        Active => ("\u{25cf}", "Active", Style::default().fg(theme::PINE)),
        Demoted => ("\u{25cb}", "Demoted", Style::default().fg(theme::GOLD)),
        Retired => ("\u{25cb}", "Retired", Style::default().fg(theme::LOVE)),
        Archived => ("\u{25cb}", "Archived", Style::default().fg(theme::MUTED)),
    }
}

/// Return a styled span for an evaluation decision string.
fn decision_styled(decision: &str) -> Vec<Span<'static>> {
    match decision.to_lowercase().as_str() {
        "promote" => vec![Span::styled(
            "\u{2713}Promote".to_string(),
            Style::default().fg(theme::FOAM),
        )],
        "demote" => vec![Span::styled(
            "\u{2717}Demote".to_string(),
            Style::default().fg(theme::LOVE),
        )],
        "retire" => vec![Span::styled(
            "\u{2717}Retire".to_string(),
            Style::default()
                .fg(theme::LOVE)
                .add_modifier(Modifier::BOLD),
        )],
        "hold" => vec![Span::styled(
            "\u{2014}Hold".to_string(),
            Style::default().fg(theme::MUTED),
        )],
        _ => vec![Span::styled(
            decision.to_string(),
            theme::text(),
        )],
    }
}

// Re-export the lifecycle enum so render functions can reference it directly
use crate::app::StrategyLifecycle;
