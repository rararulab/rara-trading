#![allow(clippy::result_large_err)]
//! TUI client for the rara-trading dashboard.
//!
//! Built on ratatui + crossterm, connects to the gRPC server to display
//! live system status, events, and strategy management.

pub mod app;
pub mod error;
pub mod event_loop;
pub mod server_process;
pub mod tabs;
pub mod theme;
pub mod ui;
