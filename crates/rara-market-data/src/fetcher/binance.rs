//! Binance historical kline fetcher.
//!
//! Uses the public `/api/v3/klines` endpoint (no authentication required).
//! Paginates at 1000 candles per request (~16.6 hours of 1m data).
//! Resumes from the latest stored candle via `MAX(ts)` query.

use async_trait::async_trait;
use chrono::{DateTime, Days, NaiveDate, NaiveTime, Utc};
use snafu::ResultExt;
use tracing::info;

use super::{HistoryFetcher, HttpSnafu, ParseSnafu, Result, StoreSnafu};
use crate::store::{MarketStore, candle::CandleRow};

/// Maximum candles per Binance klines request.
const PAGE_LIMIT: u64 = 1000;

/// Binance public API base URL.
const BASE_URL: &str = "https://api.binance.com";

/// Raw candle parsed from Binance JSON before DB insertion.
struct RawKline {
    open_time_ms: i64,
    open:         f64,
    high:         f64,
    low:          f64,
    close:        f64,
    volume:       f64,
    trade_count:  u32,
}

/// Fetches historical 1m klines from Binance public API.
pub struct BinanceFetcher {
    /// HTTP client.
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

    /// Fetch one page of klines.
    async fn fetch_page(&self, start_ms: i64, end_ms: i64) -> Result<Vec<RawKline>> {
        let url = format!(
            "{BASE_URL}/api/v3/klines?symbol={}&interval=1m&startTime={start_ms}&endTime={end_ms}&\
             limit={PAGE_LIMIT}",
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

        let rows = resp
            .json::<Vec<Vec<serde_json::Value>>>()
            .await
            .context(HttpSnafu)?;
        rows.iter().map(|row| parse_binance_kline(row)).collect()
    }
}

impl BinanceFetcher {
    /// Fetch and store candles with a per-page progress callback.
    ///
    /// `on_progress` is called after each page with the number of candles
    /// written in that batch, enabling progress bar integration.
    pub async fn fetch_and_store_with_progress(
        &self,
        store: &MarketStore,
        instrument_id: &str,
        start: NaiveDate,
        end: NaiveDate,
        on_progress: impl Fn(usize) + Send + Sync,
    ) -> Result<usize> {
        Self::fetch_core(self, store, instrument_id, start, end, &on_progress).await
    }

    /// Core fetch loop shared by trait impl and progress variant.
    async fn fetch_core(
        &self,
        store: &MarketStore,
        instrument_id: &str,
        start: NaiveDate,
        end: NaiveDate,
        on_progress: &(dyn Fn(usize) + Send + Sync),
    ) -> Result<usize> {
        let range_start_ms = start.and_time(NaiveTime::MIN).and_utc().timestamp_millis();
        let range_end_ms = end
            .checked_add_days(Days::new(1))
            .expect("date overflow")
            .and_time(NaiveTime::MIN)
            .and_utc()
            .timestamp_millis()
            - 1;

        // Resume from last stored candle + 1 minute
        let resume_ms = store
            .max_ts(instrument_id, "1m")
            .await
            .context(StoreSnafu)?
            .map_or(i64::MIN, |ts| ts.timestamp_millis() + 60_000);

        let fetch_start_ms = range_start_ms.max(resume_ms);
        if fetch_start_ms > range_end_ms {
            info!("binance: already up to date, nothing to fetch");
            return Ok(0);
        }

        let mut total = 0usize;
        let mut cursor_ms = fetch_start_ms;

        while cursor_ms <= range_end_ms {
            let page = self.fetch_page(cursor_ms, range_end_ms).await?;
            if page.is_empty() {
                break;
            }

            cursor_ms = page.last().expect("non-empty page").open_time_ms + 60_001;

            let candle_rows: Vec<CandleRow> = page
                .iter()
                .map(|k| CandleRow {
                    ts:            DateTime::from_timestamp_millis(k.open_time_ms)
                        .unwrap_or(DateTime::<Utc>::MIN_UTC),
                    instrument_id: instrument_id.to_string(),
                    interval:      "1m".to_string(),
                    open:          k.open,
                    high:          k.high,
                    low:           k.low,
                    close:         k.close,
                    volume:        k.volume,
                    trade_count:   k.trade_count.cast_signed(),
                })
                .collect();

            let count = store
                .insert_candles(&candle_rows)
                .await
                .context(StoreSnafu)?;
            let written = usize::try_from(count).expect("candle count fits in usize");
            total += written;
            on_progress(written);
        }

        info!(total, "binance: fetch complete");
        Ok(total)
    }
}

/// Search Binance for tradeable USDT-margined spot symbols.
///
/// Fetches `/api/v3/exchangeInfo` and filters by `query` substring
/// (case-insensitive). Returns matching symbol names.
pub async fn search_symbols(query: &str) -> Result<Vec<String>> {
    let url = format!("{BASE_URL}/api/v3/exchangeInfo?permissions=SPOT");
    let client = reqwest::Client::new();
    let resp = client.get(&url).send().await.context(HttpSnafu)?;
    let body: serde_json::Value = resp.json().await.context(HttpSnafu)?;

    let query_upper = query.to_uppercase();
    let symbols = body["symbols"]
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .filter_map(|s| {
            let symbol = s["symbol"].as_str()?;
            let status = s["status"].as_str()?;
            let quote = s["quoteAsset"].as_str()?;
            if status == "TRADING" && quote == "USDT" && symbol.contains(&query_upper) {
                Some(symbol.to_string())
            } else {
                None
            }
        })
        .collect();

    Ok(symbols)
}

#[async_trait]
impl HistoryFetcher for BinanceFetcher {
    async fn fetch_and_store(
        &self,
        store: &MarketStore,
        instrument_id: &str,
        start: NaiveDate,
        end: NaiveDate,
    ) -> Result<usize> {
        Self::fetch_core(self, store, instrument_id, start, end, &|_| {}).await
    }
}

/// Parse a single Binance kline JSON array.
fn parse_binance_kline(row: &[serde_json::Value]) -> Result<RawKline> {
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

    let trade_count = row.get(8).and_then(serde_json::Value::as_u64).unwrap_or(0);

    Ok(RawKline {
        open_time_ms,
        open: parse_f64(1, "open")?,
        high: parse_f64(2, "high")?,
        low: parse_f64(3, "low")?,
        close: parse_f64(4, "close")?,
        volume: parse_f64(5, "volume")?,
        trade_count: u32::try_from(trade_count).unwrap_or(u32::MAX),
    })
}
