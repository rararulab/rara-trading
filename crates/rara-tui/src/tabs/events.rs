//! Events tab — real-time event stream with topic filtering and search.
//!
//! Displays system events in a `tail -f` style view with topic-based color
//! coding, pause/resume auto-scroll, keyboard navigation, and text search.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table, Wrap};
use ratatui::Frame;

use crate::app::{EventFilter, EventsState};
use crate::theme;

/// Map a topic string to its display color.
fn topic_color(topic: &str) -> ratatui::style::Color {
    match topic.to_lowercase().as_str() {
        "trading" => theme::PINE,
        "research" => theme::IRIS,
        "feedback" => theme::FOAM,
        "sentinel" => theme::GOLD,
        _ => theme::TEXT,
    }
}

/// Render the filter bar at the top of the events tab.
fn render_filter_bar(frame: &mut Frame, state: &EventsState, area: Rect) {
    let filters = [
        ("a", "All", EventFilter::All),
        ("t", "Trading", EventFilter::Trading),
        ("r", "Research", EventFilter::Research),
        ("f", "Feedback", EventFilter::Feedback),
        ("s", "Sentinel", EventFilter::Sentinel),
    ];

    let mut spans = vec![Span::styled(" Filter: ", theme::muted())];

    for (key, label, variant) in &filters {
        let is_active = std::mem::discriminant(&state.filter) == std::mem::discriminant(variant);
        let style = if is_active {
            theme::emphasis().add_modifier(Modifier::REVERSED)
        } else {
            theme::muted()
        };
        spans.push(Span::styled(format!("[{key}]"), theme::info()));
        spans.push(Span::styled(format!("{label} "), style));
    }

    // Show search query if active
    if state.search_active {
        spans.push(Span::styled(" │ ", theme::muted()));
        spans.push(Span::styled("/", theme::warning()));
        spans.push(Span::styled(&state.search_query, theme::text()));
        spans.push(Span::styled("_", theme::warning()));
    } else if !state.search_query.is_empty() {
        spans.push(Span::styled(" │ ", theme::muted()));
        spans.push(Span::styled(
            format!("search: {}", state.search_query),
            theme::muted(),
        ));
    }

    let bar = Paragraph::new(Line::from(spans)).style(Style::default().bg(theme::SURFACE));
    frame.render_widget(bar, area);
}

/// Render the event list table.
fn render_event_list(frame: &mut Frame, state: &EventsState, area: Rect) {
    let filtered = filtered_events(state);

    let header = Row::new(vec![
        Cell::from("Seq").style(theme::muted()),
        Cell::from("Time").style(theme::muted()),
        Cell::from("Topic").style(theme::muted()),
        Cell::from("Type").style(theme::muted()),
        Cell::from("Summary").style(theme::muted()),
    ])
    .height(1);

    let rows: Vec<Row> = filtered
        .iter()
        .enumerate()
        .map(|(i, entry)| {
            let color = topic_color(&entry.topic);
            let row_style = if i == state.selected_index {
                Style::default().fg(color).add_modifier(Modifier::REVERSED)
            } else {
                Style::default().fg(color)
            };
            Row::new(vec![
                Cell::from(entry.seq.to_string()),
                Cell::from(entry.time.as_str()),
                Cell::from(entry.topic.as_str()),
                Cell::from(entry.event_type.as_str()),
                Cell::from(entry.summary.as_str()),
            ])
            .style(row_style)
        })
        .collect();

    let paused_title = if state.auto_scroll {
        " Events "
    } else {
        " Events ─ ─ ─ PAUSED ─ ─ ─ "
    };

    let table = Table::new(
        rows,
        [
            Constraint::Length(6),
            Constraint::Length(12),
            Constraint::Length(10),
            Constraint::Length(14),
            Constraint::Min(20),
        ],
    )
    .header(header)
    .block(
        Block::default()
            .title(paused_title)
            .borders(Borders::ALL)
            .border_style(theme::muted()),
    );

    frame.render_widget(table, area);
}

/// Render the detail pane showing the selected event's payload.
fn render_detail_pane(frame: &mut Frame, state: &EventsState, area: Rect) {
    let filtered = filtered_events(state);
    let detail_text = filtered.get(state.selected_index).map_or_else(
        || "No event selected".to_string(),
        |entry| {
            use std::fmt::Write;
            let mut text = format!(
                "Seq: {}  Time: {}  Topic: {}  Type: {}\n",
                entry.seq, entry.time, entry.topic, entry.event_type
            );
            if let Some(sid) = &entry.strategy_id {
                let _ = writeln!(text, "Strategy: {sid}");
            }
            let _ = write!(text, "\n{}", entry.payload);
            text
        },
    );

    let detail = Paragraph::new(detail_text)
        .wrap(Wrap { trim: false })
        .style(theme::text())
        .block(
            Block::default()
                .title(" Detail ")
                .borders(Borders::ALL)
                .border_style(theme::muted()),
        );

    frame.render_widget(detail, area);
}

/// Return events filtered by current topic filter and search query.
fn filtered_events(state: &EventsState) -> Vec<&crate::app::EventEntry> {
    state
        .events
        .iter()
        .filter(|e| match state.filter {
            EventFilter::All => true,
            EventFilter::Trading => e.topic.eq_ignore_ascii_case("trading"),
            EventFilter::Research => e.topic.eq_ignore_ascii_case("research"),
            EventFilter::Feedback => e.topic.eq_ignore_ascii_case("feedback"),
            EventFilter::Sentinel => e.topic.eq_ignore_ascii_case("sentinel"),
        })
        .filter(|e| {
            if state.search_query.is_empty() {
                return true;
            }
            let q = state.search_query.to_lowercase();
            e.summary.to_lowercase().contains(&q)
                || e.topic.to_lowercase().contains(&q)
                || e.event_type.to_lowercase().contains(&q)
                || e.payload.to_lowercase().contains(&q)
        })
        .collect()
}

/// Render the full events tab into the given area.
pub fn render(frame: &mut Frame, state: &EventsState, area: Rect) {
    let detail_height = if state.detail_expanded { 10 } else { 0 };

    let chunks = Layout::vertical([
        Constraint::Length(1),                    // filter bar
        Constraint::Min(5),                       // event list
        Constraint::Length(detail_height),         // detail pane
    ])
    .split(area);

    render_filter_bar(frame, state, chunks[0]);
    render_event_list(frame, state, chunks[1]);

    if state.detail_expanded {
        render_detail_pane(frame, state, chunks[2]);
    }
}

/// Count of events matching current filters (used for navigation bounds).
pub fn filtered_count(state: &EventsState) -> usize {
    filtered_events(state).len()
}
