//! Research tab — tracks research loop progress and hypothesis evolution.
//!
//! Layout:
//! ```text
//! ┌─ Progress Bar ───────────────────────────────────────────────┐
//! │ [7/20] [████████████░░░░░░░░] Backtesting · momentum-rev-v2 │
//! ├─────────────────────┬────────────────────────────────────────┤
//! │ Hypothesis List     │  Backtest Results (top-right)          │
//! │  ✓ momentum-v1      │  ───────────────────────────────       │
//! │  ✗ mean-rev-v1      │  timeframe | sharpe | dd | wr | #     │
//! │ >⏳ momentum-rev-v2 │                                       │
//! │                     ├────────────────────────────────────────┤
//! │                     │  SOTA Info (bottom-right)              │
//! ├─────────────────────┘                                        │
//! │ Detail: hypothesis description text                          │
//! └──────────────────────────────────────────────────────────────┘
//! ```
//!
//! Keys: `j/k` navigate list, `p` toggle DAG popup, `Esc` close popup.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, Cell, Clear, Gauge, Paragraph, Row, Table, Wrap,
};
use ratatui::Frame;

use crate::theme;

// ---------------------------------------------------------------------------
// Data types
// ---------------------------------------------------------------------------

/// Phase of the research loop for a single hypothesis.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResearchPhase {
    /// Strategy code is being generated.
    Coding,
    /// Strategy is compiling.
    Compiling,
    /// Back-test is running.
    Backtesting,
    /// Results are being evaluated against SOTA.
    Evaluating,
    /// Hypothesis completed successfully.
    Done,
    /// Hypothesis failed.
    Failed,
}

impl ResearchPhase {
    /// Return the theme color for this phase.
    #[must_use]
    pub const fn color(&self) -> ratatui::style::Color {
        match self {
            Self::Coding => theme::PINE,
            Self::Compiling => theme::GOLD,
            Self::Backtesting => theme::IRIS,
            Self::Evaluating => theme::ROSE,
            Self::Done => theme::FOAM,
            Self::Failed => theme::LOVE,
        }
    }

    /// Human-readable label.
    #[must_use]
    pub const fn label(&self) -> &'static str {
        match self {
            Self::Coding => "Coding",
            Self::Compiling => "Compiling",
            Self::Backtesting => "Backtesting",
            Self::Evaluating => "Evaluating",
            Self::Done => "Done",
            Self::Failed => "Failed",
        }
    }
}

/// Status of a hypothesis after evaluation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HypothesisStatus {
    /// Currently being processed.
    InProgress,
    /// Accepted — beat SOTA or met acceptance criteria.
    Accepted,
    /// Rejected — did not meet criteria.
    Rejected,
}

/// A single hypothesis entry in the research loop.
#[derive(Debug, Clone)]
pub struct HypothesisEntry {
    /// Unique hypothesis ID.
    pub id: u32,
    /// Short name / label.
    pub name: String,
    /// Current status.
    pub status: HypothesisStatus,
    /// Parent hypothesis ID (for DAG lineage).
    pub parent_id: Option<u32>,
    /// Optional longer description.
    pub description: String,
}

/// A single row in the backtest results table.
#[derive(Debug, Clone)]
pub struct BacktestEntry {
    /// Timeframe label (e.g. "1h", "4h", "1d").
    pub timeframe: String,
    /// Annualized Sharpe ratio.
    pub sharpe: f64,
    /// Maximum drawdown percentage.
    pub max_drawdown: f64,
    /// Win rate percentage.
    pub win_rate: f64,
    /// Total number of trades.
    pub trade_count: u32,
}

/// Best-known strategy info (State Of The Art).
#[derive(Debug, Clone)]
pub struct SotaInfo {
    /// Name of the current SOTA strategy.
    pub name: String,
    /// Sharpe ratio of SOTA.
    pub sharpe: f64,
    /// Max drawdown of SOTA.
    pub max_drawdown: f64,
}

/// Overall research loop progress.
#[derive(Debug, Clone)]
pub struct ResearchProgress {
    /// Current hypothesis index (1-based).
    pub current: u32,
    /// Total planned hypotheses.
    pub total: u32,
    /// Current phase of the active hypothesis.
    pub phase: ResearchPhase,
    /// Name of the active hypothesis.
    pub hypothesis_name: String,
}

/// Full state for the Research tab.
#[derive(Debug, Clone)]
pub struct ResearchState {
    /// Overall loop progress.
    pub progress: ResearchProgress,
    /// All hypotheses explored so far.
    pub hypotheses: Vec<HypothesisEntry>,
    /// Index of the currently selected hypothesis in the list.
    pub selected_index: usize,
    /// Backtest results for the selected hypothesis.
    pub backtests: Vec<BacktestEntry>,
    /// Current best strategy info.
    pub sota: Option<SotaInfo>,
    /// Whether the DAG popup is visible.
    pub show_dag: bool,
}

impl ResearchState {
    /// Create a default (empty) research state.
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            progress: ResearchProgress {
                current: 0,
                total: 0,
                phase: ResearchPhase::Coding,
                hypothesis_name: String::new(),
            },
            hypotheses: Vec::new(),
            selected_index: 0,
            backtests: Vec::new(),
            sota: None,
            show_dag: false,
        }
    }

    /// Move selection up in the hypothesis list.
    pub const fn select_prev(&mut self) {
        if self.selected_index > 0 {
            self.selected_index -= 1;
        }
    }

    /// Move selection down in the hypothesis list.
    pub const fn select_next(&mut self) {
        if !self.hypotheses.is_empty() && self.selected_index < self.hypotheses.len() - 1 {
            self.selected_index += 1;
        }
    }

    /// Toggle the DAG popup visibility.
    pub const fn toggle_dag(&mut self) {
        self.show_dag = !self.show_dag;
    }

    /// Close the DAG popup if open.
    pub const fn close_dag(&mut self) {
        self.show_dag = false;
    }
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

/// Render the full Research tab into the given area.
pub fn render(frame: &mut Frame, state: &ResearchState, area: Rect) {
    let layout = Layout::vertical([
        Constraint::Length(3), // progress bar
        Constraint::Min(8),   // main body
        Constraint::Length(5), // detail pane
    ])
    .split(area);

    render_progress(frame, state, layout[0]);
    render_body(frame, state, layout[1]);
    render_detail(frame, state, layout[2]);

    if state.show_dag {
        render_dag_popup(frame, state, area);
    }
}

/// Render the progress bar at the top.
fn render_progress(frame: &mut Frame, state: &ResearchState, area: Rect) {
    let p = &state.progress;
    let ratio = if p.total > 0 {
        f64::from(p.current) / f64::from(p.total)
    } else {
        0.0
    };

    // Build lineage suffix: (#id <- #parent)
    let lineage = state
        .hypotheses
        .iter()
        .find(|h| h.name == p.hypothesis_name)
        .and_then(|h| {
            h.parent_id
                .map(|pid| format!(" (#{}<-#{})", h.id, pid))
        })
        .unwrap_or_default();

    let label = format!(
        "[{}/{}] {} · {}{}",
        p.current,
        p.total,
        p.phase.label(),
        p.hypothesis_name,
        lineage,
    );

    let gauge = Gauge::default()
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(theme::muted())
                .title(" Progress "),
        )
        .gauge_style(Style::default().fg(p.phase.color()).bg(theme::SURFACE))
        .ratio(ratio.clamp(0.0, 1.0))
        .label(label);

    frame.render_widget(gauge, area);
}

/// Render the main body: hypothesis list (left) and results (right).
fn render_body(frame: &mut Frame, state: &ResearchState, area: Rect) {
    let cols = Layout::horizontal([
        Constraint::Percentage(35), // hypothesis list
        Constraint::Percentage(65), // right pane
    ])
    .split(area);

    render_hypothesis_list(frame, state, cols[0]);

    let right = Layout::vertical([
        Constraint::Percentage(60), // backtest results
        Constraint::Percentage(40), // SOTA info
    ])
    .split(cols[1]);

    render_backtest_table(frame, state, right[0]);
    render_sota(frame, state, right[1]);
}

/// Render the hypothesis list with status icons.
fn render_hypothesis_list(frame: &mut Frame, state: &ResearchState, area: Rect) {
    let rows: Vec<Row> = state
        .hypotheses
        .iter()
        .enumerate()
        .map(|(i, h)| {
            let (icon, style) = match h.status {
                HypothesisStatus::InProgress => (
                    "\u{23f3}",
                    Style::default().fg(theme::GOLD),
                ),
                HypothesisStatus::Accepted => (
                    "\u{2713}",
                    Style::default().fg(theme::FOAM),
                ),
                HypothesisStatus::Rejected => (
                    "\u{2717}",
                    Style::default().fg(theme::LOVE),
                ),
            };

            let selected_marker = if i == state.selected_index { ">" } else { " " };

            let row = Row::new(vec![
                Cell::from(selected_marker).style(theme::emphasis()),
                Cell::from(icon).style(style),
                Cell::from(format!("#{} {}", h.id, h.name)).style(style),
            ]);

            if i == state.selected_index {
                row.style(Style::default().bg(theme::OVERLAY).add_modifier(Modifier::BOLD))
            } else {
                row
            }
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Length(1),
            Constraint::Length(2),
            Constraint::Min(10),
        ],
    )
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(theme::muted())
            .title(" Hypotheses "),
    );

    frame.render_widget(table, area);
}

/// Render the backtest results table for the selected hypothesis.
fn render_backtest_table(frame: &mut Frame, state: &ResearchState, area: Rect) {
    let header = Row::new(vec!["Timeframe", "Sharpe", "MaxDD%", "WinRate%", "Trades"])
        .style(theme::emphasis())
        .bottom_margin(1);

    let rows: Vec<Row> = state
        .backtests
        .iter()
        .map(|b| {
            Row::new(vec![
                Cell::from(b.timeframe.clone()),
                Cell::from(format!("{:.2}", b.sharpe)),
                Cell::from(format!("{:.1}%", b.max_drawdown)),
                Cell::from(format!("{:.1}%", b.win_rate)),
                Cell::from(b.trade_count.to_string()),
            ])
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Length(10),
            Constraint::Length(8),
            Constraint::Length(8),
            Constraint::Length(9),
            Constraint::Length(8),
        ],
    )
    .header(header)
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(theme::muted())
            .title(" Backtest Results "),
    );

    frame.render_widget(table, area);
}

/// Render the SOTA information panel.
fn render_sota(frame: &mut Frame, state: &ResearchState, area: Rect) {
    let text = state.sota.as_ref().map_or_else(
        || "No SOTA established yet.".to_string(),
        |s| {
            format!(
                "Strategy: {}\nSharpe:   {:.2}\nMaxDD:    {:.1}%",
                s.name, s.sharpe, s.max_drawdown,
            )
        },
    );

    let paragraph = Paragraph::new(text)
        .style(theme::text())
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(theme::muted())
                .title(" SOTA "),
        );

    frame.render_widget(paragraph, area);
}

/// Render the detail pane at the bottom showing selected hypothesis description.
fn render_detail(frame: &mut Frame, state: &ResearchState, area: Rect) {
    let text = state
        .hypotheses
        .get(state.selected_index)
        .map_or_else(
            || "No hypothesis selected.".to_string(),
            |h| {
                if h.description.is_empty() {
                    format!("#{} {} — no description available.", h.id, h.name)
                } else {
                    format!("#{} {} — {}", h.id, h.name, h.description)
                }
            },
        );

    let paragraph = Paragraph::new(text)
        .style(theme::text())
        .wrap(Wrap { trim: true })
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(theme::muted())
                .title(" Detail "),
        );

    frame.render_widget(paragraph, area);
}

/// Render a centered popup showing the hypothesis evolution DAG.
fn render_dag_popup(frame: &mut Frame, state: &ResearchState, area: Rect) {
    let popup = centered_rect(60, 70, area);

    frame.render_widget(Clear, popup);

    let mut lines: Vec<Line> = Vec::new();

    // Build a simple text-based DAG showing parent -> child relationships.
    // Roots are hypotheses with no parent.
    let roots: Vec<&HypothesisEntry> = state
        .hypotheses
        .iter()
        .filter(|h| h.parent_id.is_none())
        .collect();

    for root in &roots {
        render_dag_node(&mut lines, root, state, 0);
    }

    if lines.is_empty() {
        lines.push(Line::from(Span::styled(
            "No hypotheses yet.",
            theme::muted(),
        )));
    }

    let paragraph = Paragraph::new(lines)
        .style(theme::text())
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(theme::IRIS))
                .style(Style::default().bg(theme::OVERLAY))
                .title(" Hypothesis DAG (p to close) "),
        );

    frame.render_widget(paragraph, popup);
}

/// Recursively render a DAG node and its children as indented text lines.
fn render_dag_node(
    lines: &mut Vec<Line<'_>>,
    node: &HypothesisEntry,
    state: &ResearchState,
    depth: usize,
) {
    let indent = "  ".repeat(depth);
    let connector = if depth > 0 { "└─ " } else { "" };
    let icon = match node.status {
        HypothesisStatus::InProgress => "\u{23f3}",
        HypothesisStatus::Accepted => "\u{2713}",
        HypothesisStatus::Rejected => "\u{2717}",
    };
    let color = match node.status {
        HypothesisStatus::InProgress => theme::GOLD,
        HypothesisStatus::Accepted => theme::FOAM,
        HypothesisStatus::Rejected => theme::LOVE,
    };

    lines.push(Line::from(vec![
        Span::styled(format!("{indent}{connector}"), theme::muted()),
        Span::styled(format!("{icon} "), Style::default().fg(color)),
        Span::styled(
            format!("#{} {}", node.id, node.name),
            Style::default().fg(color),
        ),
    ]));

    // Find children of this node
    let children: Vec<&HypothesisEntry> = state
        .hypotheses
        .iter()
        .filter(|h| h.parent_id == Some(node.id))
        .collect();

    for child in &children {
        render_dag_node(lines, child, state, depth + 1);
    }
}

/// Compute a centered rectangle within `area` using percentage width and height.
fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let popup_layout = Layout::vertical([
        Constraint::Percentage((100 - percent_y) / 2),
        Constraint::Percentage(percent_y),
        Constraint::Percentage((100 - percent_y) / 2),
    ])
    .split(area);

    Layout::horizontal([
        Constraint::Percentage((100 - percent_x) / 2),
        Constraint::Percentage(percent_x),
        Constraint::Percentage((100 - percent_x) / 2),
    ])
    .split(popup_layout[1])[1]
}
