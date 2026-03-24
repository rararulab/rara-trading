//! Market data loading for backtesting.
//!
//! Provides utilities for loading historical market data from JSON files
//! and converting it into barter-compatible `MarketStreamEvent` streams.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use barter::backtest::market_data::MarketDataInMemory;
use barter_data::event::{DataKind, MarketEvent};
use barter_data::streams::reconnect::Event as ReconnectEvent;
use barter_data::subscription::candle::Candle;
use barter_data::subscription::trade::PublicTrade;
use barter_instrument::exchange::ExchangeId;
use barter_instrument::instrument::InstrumentIndex;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use snafu::Snafu;

/// Errors from market data loading operations.
#[derive(Debug, Snafu)]
#[snafu(visibility(pub))]
pub enum MarketDataError {
    /// Failed to read a market data file from disk.
    #[snafu(display("failed to read market data file {path}: {source}"))]
    ReadFile {
        /// Path to the file that could not be read.
        path: String,
        /// Underlying IO error.
        source: std::io::Error,
    },

    /// Failed to parse market data JSON.
    #[snafu(display("failed to parse market data JSON from {path}: {source}"))]
    ParseJson {
        /// Path to the file with invalid JSON.
        path: String,
        /// Underlying serde error.
        source: serde_json::Error,
    },

    /// No market data files found in the specified directory.
    #[snafu(display("no market data files found in {directory}"))]
    NoDataFiles {
        /// Directory that was searched.
        directory: String,
    },

    /// Market data is empty after loading.
    #[snafu(display("loaded market data is empty"))]
    EmptyData,
}

/// Result type for market data operations.
pub type Result<T> = std::result::Result<T, MarketDataError>;

/// A single OHLCV candle record as stored in JSON data files.
///
/// This is the on-disk format; it gets converted to barter's `MarketEvent<DataKind>`
/// for use in the backtest engine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CandleRecord {
    /// Candle close time as RFC3339 or Unix timestamp.
    pub time: DateTime<Utc>,
    /// Opening price.
    pub open: f64,
    /// Highest price.
    pub high: f64,
    /// Lowest price.
    pub low: f64,
    /// Closing price.
    pub close: f64,
    /// Trading volume.
    pub volume: f64,
    /// Number of trades in the candle period (optional, defaults to 0).
    #[serde(default)]
    pub trade_count: u64,
}

/// A single trade record as stored in JSON data files.
///
/// This is the on-disk format; it gets converted to barter's `MarketEvent<DataKind>`
/// for use in the backtest engine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TradeRecord {
    /// Trade execution time.
    pub time: DateTime<Utc>,
    /// Trade ID.
    #[serde(default)]
    pub id: String,
    /// Trade price.
    pub price: f64,
    /// Trade amount/quantity.
    pub amount: f64,
    /// Trade side: "buy" or "sell".
    pub side: String,
}

/// Load candle records from a JSON file and convert them to barter `MarketStreamEvent`s.
///
/// Each candle is wrapped as a `DataKind::Candle` market event with the given instrument index
/// and exchange ID.
pub fn load_candles_from_file(
    path: &Path,
    instrument_index: InstrumentIndex,
    exchange: ExchangeId,
) -> Result<Vec<ReconnectEvent<ExchangeId, MarketEvent<InstrumentIndex, DataKind>>>> {
    let content = std::fs::read_to_string(path).map_err(|source| MarketDataError::ReadFile {
        path: path.display().to_string(),
        source,
    })?;

    let records: Vec<CandleRecord> =
        serde_json::from_str(&content).map_err(|source| MarketDataError::ParseJson {
            path: path.display().to_string(),
            source,
        })?;

    let events = records
        .into_iter()
        .map(|record| {
            let candle = Candle {
                close_time: record.time,
                open: record.open,
                high: record.high,
                low: record.low,
                close: record.close,
                volume: record.volume,
                trade_count: record.trade_count,
            };

            let market_event = MarketEvent {
                time_exchange: record.time,
                time_received: record.time,
                exchange,
                instrument: instrument_index,
                kind: DataKind::Candle(candle),
            };

            ReconnectEvent::Item(market_event)
        })
        .collect();

    Ok(events)
}

/// Load trade records from a JSON file and convert them to barter `MarketStreamEvent`s.
///
/// Each trade is wrapped as a `DataKind::Trade` market event with the given instrument index
/// and exchange ID.
pub fn load_trades_from_file(
    path: &Path,
    instrument_index: InstrumentIndex,
    exchange: ExchangeId,
) -> Result<Vec<ReconnectEvent<ExchangeId, MarketEvent<InstrumentIndex, DataKind>>>> {
    let content = std::fs::read_to_string(path).map_err(|source| MarketDataError::ReadFile {
        path: path.display().to_string(),
        source,
    })?;

    let records: Vec<TradeRecord> =
        serde_json::from_str(&content).map_err(|source| MarketDataError::ParseJson {
            path: path.display().to_string(),
            source,
        })?;

    let events = records
        .into_iter()
        .map(|record| {
            let side = if record.side.eq_ignore_ascii_case("buy") {
                barter_instrument::Side::Buy
            } else {
                barter_instrument::Side::Sell
            };

            let trade = PublicTrade {
                id: record.id,
                price: record.price,
                amount: record.amount,
                side,
            };

            let market_event = MarketEvent {
                time_exchange: record.time,
                time_received: record.time,
                exchange,
                instrument: instrument_index,
                kind: DataKind::Trade(trade),
            };

            ReconnectEvent::Item(market_event)
        })
        .collect();

    Ok(events)
}

/// Load all market data files from a directory for a given contract.
///
/// Scans the directory for JSON files matching the contract ID pattern,
/// loads them as either candle or trade data, and returns an in-memory
/// market data source suitable for barter backtesting.
///
/// File naming convention:
/// - `{contract_id}_candles.json` for OHLCV candle data
/// - `{contract_id}_trades.json` for trade data
pub fn load_market_data_for_contract(
    data_dir: &Path,
    contract_id: &str,
    instrument_index: InstrumentIndex,
    exchange: ExchangeId,
) -> Result<MarketDataInMemory<DataKind>> {
    let candle_path = data_dir.join(format!("{contract_id}_candles.json"));
    let trade_path = data_dir.join(format!("{contract_id}_trades.json"));

    let mut all_events = Vec::new();

    if candle_path.exists() {
        let candle_events = load_candles_from_file(&candle_path, instrument_index, exchange)?;
        all_events.extend(candle_events);
    }

    if trade_path.exists() {
        let trade_events = load_trades_from_file(&trade_path, instrument_index, exchange)?;
        all_events.extend(trade_events);
    }

    if all_events.is_empty() {
        // Try loading any JSON file matching the contract_id prefix
        let generic_path = data_dir.join(format!("{contract_id}.json"));
        if generic_path.exists() {
            // Attempt candle format first, fall back to trades
            if let Ok(events) =
                load_candles_from_file(&generic_path, instrument_index, exchange)
            {
                all_events.extend(events);
            } else {
                let events =
                    load_trades_from_file(&generic_path, instrument_index, exchange)?;
                all_events.extend(events);
            }
        }
    }

    if all_events.is_empty() {
        return Err(MarketDataError::NoDataFiles {
            directory: data_dir.display().to_string(),
        });
    }

    // Sort events by exchange time so the backtest processes them chronologically
    all_events.sort_by(|a, b| {
        let time_a = match a {
            ReconnectEvent::Item(event) => event.time_exchange,
            ReconnectEvent::Reconnecting(_) => DateTime::<Utc>::MIN_UTC,
        };
        let time_b = match b {
            ReconnectEvent::Item(event) => event.time_exchange,
            ReconnectEvent::Reconnecting(_) => DateTime::<Utc>::MIN_UTC,
        };
        time_a.cmp(&time_b)
    });

    Ok(MarketDataInMemory::new(Arc::new(all_events)))
}

/// Resolve the data directory path, creating it if it does not exist.
pub fn resolve_data_dir(data_dir: &Path) -> Result<PathBuf> {
    if !data_dir.exists() {
        std::fs::create_dir_all(data_dir).map_err(|source| MarketDataError::ReadFile {
            path: data_dir.display().to_string(),
            source,
        })?;
    }
    Ok(data_dir.to_path_buf())
}
