//! Binance historical kline fetcher.
//!
//! Uses the public `/api/v3/klines` endpoint (no authentication required).
//! Paginates at 1000 candles per request (~16.6 hours of 1m data).

use std::path::Path;

use async_trait::async_trait;
use chrono::{Days, NaiveDate, NaiveTime};
use snafu::ResultExt;
use tracing::info;

use super::{HistoryFetcher, HttpSnafu, ParseSnafu, Result, store_day};
use crate::ingester::RawCandle;

/// Maximum candles per Binance klines request.
const PAGE_LIMIT: u64 = 1000;

/// Binance public API base URL.
const BASE_URL: &str = "https://api.binance.com";

/// Fetches historical 1m klines from Binance public API.
pub struct BinanceFetcher {
    pub client: reqwest::Client,
    /// Binance symbol, e.g. `"BTCUSDT"`.
    pub symbol: String,
}

impl BinanceFetcher {
    /// Create a new fetcher for the given Binance symbol.
    pub fn new(symbol: impl Into<String>) -> Self {
        Self {
            client: reqwest::Client::new(),
            symbol: symbol.into(),
        }
    }

    /// Fetch one page of klines starting at `start_ms` up to `end_ms`.
    async fn fetch_page(&self, start_ms: i64, end_ms: i64) -> Result<Vec<RawCandle>> {
        let url = format!(
            "{BASE_URL}/api/v3/klines?symbol={}&interval=1m&startTime={start_ms}&endTime={end_ms}&limit={PAGE_LIMIT}",
            self.symbol
        );
        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .context(HttpSnafu)?
            .error_for_status()
            .context(HttpSnafu)?;

        let rows = resp.json::<Vec<Vec<serde_json::Value>>>().await.context(HttpSnafu)?;

        rows.iter().map(|row| parse_binance_kline(row)).collect()
    }
}

#[async_trait]
impl HistoryFetcher for BinanceFetcher {
    async fn fetch_and_store(
        &self,
        data_dir: &Path,
        instrument_id: &str,
        start: NaiveDate,
        end: NaiveDate,
    ) -> Result<usize> {
        let mut total = 0usize;
        let mut current = start;

        while current <= end {
            let day_start_ms = current
                .and_time(NaiveTime::MIN)
                .and_utc()
                .timestamp_millis();
            let day_end_ms = day_start_ms + 86_400_000 - 1;

            let mut candles = Vec::new();
            let mut page_start = day_start_ms;

            // Paginate within the day (1440 1m candles = 2 pages of 1000)
            while page_start <= day_end_ms {
                let page = self.fetch_page(page_start, day_end_ms).await?;
                if page.is_empty() {
                    break;
                }
                page_start = page.last().expect("non-empty page").timestamp_ns / 1_000_000 + 1;
                candles.extend(page);
            }

            let count = store_day(data_dir, instrument_id, "candles_1m", current, &candles)?;
            info!(date = %current, candles = count, "binance: ingested day");
            total += count;

            current = current
                .checked_add_days(Days::new(1))
                .expect("date overflow");
        }

        Ok(total)
    }
}

/// Parse a single Binance kline JSON array into a [`RawCandle`].
///
/// Binance format: `[open_time, open, high, low, close, volume, close_time, quote_vol, trades, ...]`
fn parse_binance_kline(row: &[serde_json::Value]) -> Result<RawCandle> {
    let parse_f64 = |idx: usize, name: &str| -> Result<f64> {
        row.get(idx)
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<f64>().ok())
            .ok_or_else(|| {
                ParseSnafu {
                    message: format!("missing {name} at index {idx}"),
                }
                .build()
            })
    };

    let open_time_ms = row
        .first()
        .and_then(serde_json::Value::as_i64)
        .ok_or_else(|| {
            ParseSnafu {
                message: "missing open_time".to_string(),
            }
            .build()
        })?;

    let trade_count = row
        .get(8)
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);

    Ok(RawCandle {
        timestamp_ns: open_time_ms * 1_000_000,
        open: parse_f64(1, "open")?,
        high: parse_f64(2, "high")?,
        low: parse_f64(3, "low")?,
        close: parse_f64(4, "close")?,
        volume: parse_f64(5, "volume")?,
        trade_count: u32::try_from(trade_count).unwrap_or(u32::MAX),
    })
}
