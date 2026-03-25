//! Historical market data fetchers for various exchanges.

pub mod binance;
pub mod yahoo;

use std::path::Path;

use async_trait::async_trait;
use chrono::NaiveDate;
use snafu::{ResultExt, Snafu};

use crate::ingester::{self, RawCandle};

/// Errors from data fetching operations.
#[derive(Debug, Snafu)]
#[snafu(visibility(pub))]
pub enum FetchError {
    /// HTTP request failed.
    #[snafu(display("HTTP error: {source}"))]
    Http { source: reqwest::Error },

    /// Failed to parse exchange response.
    #[snafu(display("parse error: {message}"))]
    Parse { message: String },

    /// Ingestion into `.rara` storage failed.
    #[snafu(display("ingest error: {source}"))]
    Ingest { source: ingester::IngestError },
}

/// Convenience alias.
pub type Result<T> = std::result::Result<T, FetchError>;

/// Trait for fetching historical OHLCV candle data from an exchange.
#[async_trait]
pub trait HistoryFetcher: Send + Sync {
    /// Fetch candles for a date range and write to `.rara` files.
    ///
    /// Returns total number of candles written.
    async fn fetch_and_store(
        &self,
        data_dir: &Path,
        instrument_id: &str,
        start: NaiveDate,
        end: NaiveDate,
    ) -> Result<usize>;
}

/// Fetch candles for a single day and write to storage.
///
/// Shared helper used by all fetcher implementations.
fn store_day(
    data_dir: &Path,
    instrument_id: &str,
    data_type_dir: &str,
    date: NaiveDate,
    candles: &[RawCandle],
) -> Result<usize> {
    let date_str = date.format("%Y-%m-%d").to_string();
    ingester::write_candles_to_file(data_dir, instrument_id, data_type_dir, &date_str, candles)
        .context(IngestSnafu)
}
