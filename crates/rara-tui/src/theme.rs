//! Rosé Pine color theme and semantic style helpers for the TUI.

use ratatui::style::{Color, Modifier, Style};

// ---------------------------------------------------------------------------
// Rosé Pine (main variant) palette
// ---------------------------------------------------------------------------

/// Background base color.
pub const BASE: Color = Color::Rgb(25, 23, 36);
/// Slightly raised surface.
pub const SURFACE: Color = Color::Rgb(31, 29, 46);
/// Overlay/popup background.
pub const OVERLAY: Color = Color::Rgb(38, 35, 58);
/// Muted/disabled text.
pub const MUTED: Color = Color::Rgb(110, 106, 134);
/// Subtle secondary text.
pub const SUBTLE: Color = Color::Rgb(144, 140, 170);
/// Primary text color.
pub const TEXT: Color = Color::Rgb(224, 222, 244);
/// Red accent — errors, negative `PnL`, disconnected.
pub const LOVE: Color = Color::Rgb(235, 111, 146);
/// Yellow accent — warnings, reconnecting, hold.
pub const GOLD: Color = Color::Rgb(246, 193, 119);
/// Warm highlight color.
pub const ROSE: Color = Color::Rgb(235, 188, 186);
/// Cyan accent — info, links, IDs.
pub const PINE: Color = Color::Rgb(49, 116, 143);
/// Green-ish accent — positive `PnL`, connected.
pub const FOAM: Color = Color::Rgb(156, 207, 216);
/// Purple accent.
pub const IRIS: Color = Color::Rgb(196, 167, 231);

// ---------------------------------------------------------------------------
// Semantic style helpers
// ---------------------------------------------------------------------------

/// Style for positive indicators (connected, `PnL`+, promoted).
#[must_use]
pub fn positive() -> Style {
    Style::default().fg(FOAM)
}

/// Style for negative indicators (disconnected, `PnL`-, demoted).
#[must_use]
pub fn negative() -> Style {
    Style::default().fg(LOVE)
}

/// Style for warning indicators (reconnecting, hold).
#[must_use]
pub fn warning() -> Style {
    Style::default().fg(GOLD)
}

/// Style for informational elements (links, IDs).
#[must_use]
pub fn info() -> Style {
    Style::default().fg(PINE)
}

/// Style for primary text.
#[must_use]
pub fn text() -> Style {
    Style::default().fg(TEXT)
}

/// Style for muted/secondary text.
#[must_use]
pub fn muted() -> Style {
    Style::default().fg(MUTED)
}

/// Style for emphasized/active elements (selected tab, title).
#[must_use]
pub fn emphasis() -> Style {
    Style::default().fg(TEXT).add_modifier(Modifier::BOLD)
}

/// Style for highlighted elements.
#[must_use]
pub fn highlight() -> Style {
    Style::default().fg(ROSE)
}

/// Style for the status bar background.
#[must_use]
pub fn status_bar_bg() -> Style {
    Style::default().bg(SURFACE).fg(TEXT)
}

/// Style for the footer/help bar.
#[must_use]
pub fn footer() -> Style {
    Style::default().bg(SURFACE).fg(SUBTLE)
}
