//! Candle timeframe definitions for strategy evaluation.

use serde::{Deserialize, Serialize};
use strum::{Display, EnumString};

/// Supported candle aggregation timeframes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Display, EnumString)]
pub enum Timeframe {
    /// 1-minute candles (no aggregation).
    #[strum(serialize = "1m")]
    Min1,
    /// 5-minute candles.
    #[strum(serialize = "5m")]
    Min5,
    /// 15-minute candles.
    #[strum(serialize = "15m")]
    Min15,
    /// 1-hour candles.
    #[strum(serialize = "1h")]
    Hour1,
    /// 4-hour candles.
    #[strum(serialize = "4h")]
    Hour4,
    /// Daily candles.
    #[strum(serialize = "1d")]
    Day1,
}

impl Timeframe {
    /// Number of 1-minute candles that compose one candle of this timeframe.
    pub const fn minutes(&self) -> u32 {
        match self {
            Self::Min1 => 1,
            Self::Min5 => 5,
            Self::Min15 => 15,
            Self::Hour1 => 60,
            Self::Hour4 => 240,
            Self::Day1 => 1440,
        }
    }

    /// Duration of this timeframe in seconds.
    pub const fn seconds(&self) -> i64 { self.minutes() as i64 * 60 }
}
