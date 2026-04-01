//! Strategy API types for WASM-compiled trading strategies.
//!
//! This crate defines the interface between the native backtester/trading
//! engine and WASM-compiled strategies. All types use `f64` for WASM
//! compatibility.

use serde::{Deserialize, Serialize};

/// Current API version. Generated strategies must match this.
pub const API_VERSION: u32 = 1;

/// Market data snapshot for a single candle.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Candle {
    /// Unix timestamp in seconds.
    pub timestamp: i64,
    /// Opening price.
    pub open:      f64,
    /// Highest price.
    pub high:      f64,
    /// Lowest price.
    pub low:       f64,
    /// Closing price.
    pub close:     f64,
    /// Trading volume.
    pub volume:    f64,
}

/// Which side of the market.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Side {
    /// Go long (buy).
    Long,
    /// Go short (sell).
    Short,
}

/// Signal emitted by a strategy after processing candle data.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Signal {
    /// Enter a position with given side and strength (0.0..=1.0).
    Entry {
        /// Which side to enter.
        side:     Side,
        /// Signal strength from 0.0 to 1.0.
        strength: f64,
    },
    /// Exit the current position.
    Exit,
    /// Do nothing.
    Hold,
}

/// Stop-loss and take-profit price levels.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RiskLevels {
    /// Price at which to cut losses.
    pub stop_loss:   f64,
    /// Price at which to take profits.
    pub take_profit: f64,
}

/// Strategy metadata for identification and versioning.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StrategyMeta {
    /// Human-readable strategy name.
    pub name:        String,
    /// Strategy version (incremented on each iteration).
    pub version:     u32,
    /// API version this strategy was compiled against.
    pub api_version: u32,
    /// Brief description of the strategy.
    pub description: String,
}
