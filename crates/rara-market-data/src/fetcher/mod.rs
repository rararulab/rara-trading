//! Historical market data fetchers for various exchanges.

pub mod binance;
pub mod yahoo;

use async_trait::async_trait;
use chrono::NaiveDate;
use snafu::Snafu;

use crate::store;

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

    /// Database storage failed.
    #[snafu(display("store error: {source}"))]
    Store { source: store::StoreError },
}

/// Convenience alias.
pub type Result<T> = std::result::Result<T, FetchError>;

/// Trait for fetching historical OHLCV candle data from an exchange.
#[async_trait]
pub trait HistoryFetcher: Send + Sync {
    /// Fetch candles for a date range and write to `TimescaleDB`.
    ///
    /// Returns total number of candles written.
    async fn fetch_and_store(
        &self,
        store: &store::MarketStore,
        instrument_id: &str,
        start: NaiveDate,
        end: NaiveDate,
    ) -> Result<usize>;
}
