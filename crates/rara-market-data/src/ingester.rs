//! Converts raw exchange data to `.rara` binary files with idempotent writes.

use std::fs;
use std::path::{Path, PathBuf};

use snafu::{ResultExt, Snafu};

use crate::file::{FileError, RaraFileReader, RaraFileWriter};
use crate::record::{CandleRecord, RecordType, TickRecord, FIXED_POINT_SCALE};

// ---------------------------------------------------------------------------
// Error
// ---------------------------------------------------------------------------

/// Errors produced during data ingestion.
#[derive(Debug, Snafu)]
#[snafu(visibility(pub))]
pub enum IngestError {
    /// A `.rara` file operation failed.
    #[snafu(display("file error: {source}"))]
    File {
        /// Underlying file error.
        source: FileError,
    },

    /// Failed to create the output directory.
    #[snafu(display("failed to create directory: {source}"))]
    CreateDir {
        /// Underlying OS error.
        source: std::io::Error,
    },
}

/// Convenience alias for ingestion operations.
pub type Result<T> = std::result::Result<T, IngestError>;

// ---------------------------------------------------------------------------
// Raw types
// ---------------------------------------------------------------------------

/// Raw floating-point candle data from an exchange.
#[derive(Debug, Clone)]
pub struct RawCandle {
    /// Candle open timestamp as nanoseconds since Unix epoch.
    pub timestamp_ns: i64,
    /// Open price in floating-point.
    pub open: f64,
    /// High price in floating-point.
    pub high: f64,
    /// Low price in floating-point.
    pub low: f64,
    /// Close price in floating-point.
    pub close: f64,
    /// Volume in floating-point.
    pub volume: f64,
    /// Number of trades aggregated into this candle.
    pub trade_count: u32,
}

/// Raw floating-point trade data from an exchange.
#[derive(Debug, Clone)]
pub struct RawTrade {
    /// Trade timestamp as nanoseconds since Unix epoch.
    pub timestamp_ns: i64,
    /// Trade price in floating-point.
    pub price: f64,
    /// Trade amount in floating-point.
    pub amount: f64,
    /// Whether this was a buy (`true`) or sell (`false`).
    pub is_buy: bool,
}

// ---------------------------------------------------------------------------
// Conversion
// ---------------------------------------------------------------------------

/// Convert a floating-point value to fixed-point by multiplying by [`FIXED_POINT_SCALE`].
#[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
pub fn to_fixed(value: f64) -> i64 {
    (value * FIXED_POINT_SCALE as f64) as i64
}

// ---------------------------------------------------------------------------
// Write helpers
// ---------------------------------------------------------------------------

/// Build the output directory path and ensure it exists.
fn ensure_dir(data_dir: &Path, instrument_id: &str, data_type_dir: &str) -> Result<PathBuf> {
    let dir = data_dir
        .join("hot")
        .join(instrument_id)
        .join(data_type_dir);
    fs::create_dir_all(&dir).context(CreateDirSnafu)?;
    Ok(dir)
}

/// Read the last candle timestamp from an existing file, returning `None` if the file is empty.
fn last_candle_ts(path: &Path) -> Result<Option<i64>> {
    let reader = RaraFileReader::open(path).context(FileSnafu)?;
    reader.last_candle_timestamp().context(FileSnafu)
}

/// Read the last tick timestamp from an existing file, returning `None` if the file is empty.
fn last_tick_ts(path: &Path) -> Result<Option<i64>> {
    let reader = RaraFileReader::open(path).context(FileSnafu)?;
    reader.last_tick_timestamp().context(FileSnafu)
}

/// Write raw candles to a `.rara` file with deduplication against existing data.
///
/// Returns the number of newly written records.
pub fn write_candles_to_file(
    data_dir: &Path,
    instrument_id: &str,
    data_type_dir: &str,
    date: &str,
    candles: &[RawCandle],
) -> Result<usize> {
    let dir = ensure_dir(data_dir, instrument_id, data_type_dir)?;
    let file_path = dir.join(format!("{date}.rara"));

    let file_exists = file_path.exists();

    // Determine the dedup cutoff timestamp
    let last_ts = if file_exists {
        last_candle_ts(&file_path)?.unwrap_or(i64::MIN)
    } else {
        i64::MIN
    };

    // Filter to only new records
    let new_candles: Vec<CandleRecord> = candles
        .iter()
        .filter(|c| c.timestamp_ns > last_ts)
        .map(|c| CandleRecord {
            ts_event: c.timestamp_ns,
            open: to_fixed(c.open),
            high: to_fixed(c.high),
            low: to_fixed(c.low),
            close: to_fixed(c.close),
            volume: to_fixed(c.volume),
            trade_count: c.trade_count,
            pad: [0; 12],
        })
        .collect();

    if new_candles.is_empty() {
        return Ok(0);
    }

    let mut writer = if file_exists {
        RaraFileWriter::open_append(&file_path).context(FileSnafu)?
    } else {
        RaraFileWriter::create(&file_path, instrument_id, RecordType::Candle).context(FileSnafu)?
    };

    let count = new_candles.len();
    writer.append_candles(&new_candles).context(FileSnafu)?;
    writer.flush().context(FileSnafu)?;

    Ok(count)
}

/// Write raw trades to a `.rara` file with deduplication against existing data.
///
/// Returns the number of newly written records.
pub fn write_trades_to_file(
    data_dir: &Path,
    instrument_id: &str,
    date: &str,
    trades: &[RawTrade],
) -> Result<usize> {
    let dir = ensure_dir(data_dir, instrument_id, "ticks")?;
    let file_path = dir.join(format!("{date}.rara"));

    let file_exists = file_path.exists();

    let last_ts = if file_exists {
        last_tick_ts(&file_path)?.unwrap_or(i64::MIN)
    } else {
        i64::MIN
    };

    let new_ticks: Vec<TickRecord> = trades
        .iter()
        .filter(|t| t.timestamp_ns > last_ts)
        .map(|t| TickRecord {
            ts_event: t.timestamp_ns,
            price: to_fixed(t.price),
            amount: to_fixed(t.amount),
            side: u8::from(!t.is_buy),
            pad: [0; 7],
        })
        .collect();

    if new_ticks.is_empty() {
        return Ok(0);
    }

    let mut writer = if file_exists {
        RaraFileWriter::open_append(&file_path).context(FileSnafu)?
    } else {
        RaraFileWriter::create(&file_path, instrument_id, RecordType::Tick).context(FileSnafu)?
    };

    let count = new_ticks.len();
    writer.append_ticks(&new_ticks).context(FileSnafu)?;
    writer.flush().context(FileSnafu)?;

    Ok(count)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    use crate::file::RaraFileReader;

    fn sample_raw_candle(ts: i64) -> RawCandle {
        RawCandle {
            timestamp_ns: ts,
            open: 42_000.0,
            high: 43_000.0,
            low: 41_000.0,
            close: 42_500.0,
            volume: 100.0,
            trade_count: 10,
        }
    }

    #[test]
    fn ingest_writes_candles_to_rara_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let candles = [sample_raw_candle(1000), sample_raw_candle(2000)];

        let written =
            write_candles_to_file(dir.path(), "BTC-USDT", "candles-1m", "2024-01-01", &candles)
                .expect("write");
        assert_eq!(written, 2);

        let file_path = dir
            .path()
            .join("hot/BTC-USDT/candles-1m/2024-01-01.rara");
        let reader = RaraFileReader::open(&file_path).expect("open");
        assert_eq!(reader.header().record_count, 2);

        let records = reader.candle_records().expect("candle_records");
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].ts_event, 1000);
        assert_eq!(records[1].ts_event, 2000);
    }

    #[test]
    fn ingest_appends_idempotently() {
        let dir = tempfile::tempdir().expect("tempdir");
        let candle = sample_raw_candle(1000);

        let written =
            write_candles_to_file(dir.path(), "BTC-USDT", "candles-1m", "2024-01-01", std::slice::from_ref(&candle))
                .expect("first write");
        assert_eq!(written, 1);

        // Write the same candle again — should be deduplicated
        let written =
            write_candles_to_file(dir.path(), "BTC-USDT", "candles-1m", "2024-01-01", &[candle])
                .expect("second write");
        assert_eq!(written, 0);

        let file_path = dir
            .path()
            .join("hot/BTC-USDT/candles-1m/2024-01-01.rara");
        let reader = RaraFileReader::open(&file_path).expect("open");
        assert_eq!(reader.header().record_count, 1);
    }
}
