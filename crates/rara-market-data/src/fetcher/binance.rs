//! Binance historical kline fetcher.
//!
//! Uses the official `binance-sdk` crate to access the public Binance API.
//! Paginates at 1000 candles per request (~16.6 hours of 1m data).
//! Detects both head gaps (start < stored min) and tail gaps (stored max < end)
//! so re-runs with an earlier start date correctly backfill missing history.

use async_trait::async_trait;
use binance_sdk::{
    common::config::ConfigurationRestApi,
    spot::rest_api::{
        ExchangeInfoParams, KlinesIntervalEnum, KlinesItemInner, KlinesParams, RestApi,
    },
};
use chrono::{DateTime, Days, NaiveDate, NaiveTime, Utc};
use snafu::ResultExt;
use tracing::info;

use super::{HistoryFetcher, ParseSnafu, Result, StoreSnafu};
use crate::store::{MarketStore, candle::CandleRow};

/// Maximum candles per Binance klines request.
const PAGE_LIMIT: i32 = 1000;

/// Create a Binance REST API client (no auth needed for market data).
fn create_client() -> Result<RestApi> {
    let config = ConfigurationRestApi::builder().build().map_err(|e| {
        ParseSnafu {
            message: format!("failed to build Binance config: {e}"),
        }
        .build()
    })?;
    Ok(RestApi::new(config))
}

/// Fetches historical 1m klines from Binance using the official SDK.
pub struct BinanceFetcher {
    /// Binance REST API client.
    api:        RestApi,
    /// Binance symbol, e.g. `"BTCUSDT"`.
    pub symbol: String,
}

impl BinanceFetcher {
    /// Create a new fetcher for the given Binance symbol.
    pub fn new(symbol: impl Into<String>) -> Self {
        Self {
            api:    create_client().expect("Binance client must build"),
            symbol: symbol.into(),
        }
    }

    /// Query the earliest available kline timestamp for this symbol.
    ///
    /// Fetches a single candle starting from epoch to discover when Binance
    /// first has data for the symbol. Returns `None` if no data exists.
    pub async fn earliest_available(&self) -> Result<Option<NaiveDate>> {
        let params = KlinesParams::builder(self.symbol.clone(), KlinesIntervalEnum::Interval1m)
            .start_time(0_i64)
            .limit(1)
            .build()
            .map_err(|e| {
                ParseSnafu {
                    message: e.to_string(),
                }
                .build()
            })?;

        let resp = self.api.klines(params).await.map_err(|e| {
            ParseSnafu {
                message: format!("klines request failed: {e}"),
            }
            .build()
        })?;

        let klines = resp.data().await.map_err(|e| {
            ParseSnafu {
                message: format!("klines parse failed: {e}"),
            }
            .build()
        })?;

        Ok(klines.first().and_then(|row| {
            extract_open_time(row)
                .and_then(|ms| DateTime::from_timestamp_millis(ms).map(|dt| dt.date_naive()))
        }))
    }

    /// Fetch one page of klines via the SDK.
    async fn fetch_page(&self, start_ms: i64, end_ms: i64) -> Result<Vec<Vec<KlinesItemInner>>> {
        let params = KlinesParams::builder(self.symbol.clone(), KlinesIntervalEnum::Interval1m)
            .start_time(start_ms)
            .end_time(end_ms)
            .limit(PAGE_LIMIT)
            .build()
            .map_err(|e| {
                ParseSnafu {
                    message: e.to_string(),
                }
                .build()
            })?;

        let resp = self.api.klines(params).await.map_err(|e| {
            ParseSnafu {
                message: format!("klines request failed: {e}"),
            }
            .build()
        })?;

        let klines = resp.data().await.map_err(|e| {
            ParseSnafu {
                message: format!("klines parse failed: {e}"),
            }
            .build()
        })?;
        Ok(klines)
    }

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
        self.fetch_core(store, instrument_id, start, end, &on_progress)
            .await
    }

    /// Core fetch loop shared by trait impl and progress variant.
    ///
    /// Handles two gaps: a **head gap** (requested start < stored `min_ts`)
    /// and a **tail gap** (stored `max_ts` < requested end). Previous logic
    /// only checked `max_ts`, silently skipping all data before `min_ts`.
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

        let stored_min = store
            .min_ts(instrument_id, "1m")
            .await
            .context(StoreSnafu)?;
        let stored_max = store
            .max_ts(instrument_id, "1m")
            .await
            .context(StoreSnafu)?;

        let mut total = 0usize;

        // Head gap: requested start is before earliest stored candle
        if let Some(min_ts) = stored_min {
            let min_ms = min_ts.timestamp_millis();
            if range_start_ms < min_ms {
                let head_end = min_ms - 1; // up to just before the first stored candle
                info!("binance: head gap detected, fetching {range_start_ms} → {head_end}");
                total += self
                    .fetch_range(store, instrument_id, range_start_ms, head_end, on_progress)
                    .await?;
            }
        }

        // Tail gap: resume from last stored candle + 1 minute (or from start if no
        // data)
        let tail_start = stored_max.map_or(range_start_ms, |ts| ts.timestamp_millis() + 60_000);
        if tail_start <= range_end_ms {
            total += self
                .fetch_range(store, instrument_id, tail_start, range_end_ms, on_progress)
                .await?;
        }

        if total == 0 {
            info!("binance: already up to date, nothing to fetch");
        } else {
            info!(total, "binance: fetch complete");
        }
        Ok(total)
    }

    /// Fetch and store candles for a contiguous millisecond range.
    async fn fetch_range(
        &self,
        store: &MarketStore,
        instrument_id: &str,
        start_ms: i64,
        end_ms: i64,
        on_progress: &(dyn Fn(usize) + Send + Sync),
    ) -> Result<usize> {
        let mut total = 0usize;
        let mut cursor_ms = start_ms;

        while cursor_ms <= end_ms {
            let page = self.fetch_page(cursor_ms, end_ms).await?;
            if page.is_empty() {
                break;
            }

            let last_open_time = page
                .last()
                .and_then(|row| extract_open_time(row))
                .expect("non-empty page must have open_time");
            cursor_ms = last_open_time + 60_001;

            let candle_rows: Vec<CandleRow> = page
                .iter()
                .filter_map(|row| parse_sdk_kline(row, instrument_id))
                .collect();

            let count = store
                .insert_candles(&candle_rows)
                .await
                .context(StoreSnafu)?;
            let written = usize::try_from(count).expect("candle count fits in usize");
            total += written;
            on_progress(written);
        }

        Ok(total)
    }
}

/// Search Binance for tradeable USDT-margined spot symbols.
///
/// Uses the SDK's `exchange_info` endpoint and filters by `query` substring
/// (case-insensitive). Returns matching symbol names.
pub async fn search_symbols(query: &str) -> Result<Vec<String>> {
    let api = create_client()?;
    let params = ExchangeInfoParams::builder().build().map_err(|e| {
        ParseSnafu {
            message: e.to_string(),
        }
        .build()
    })?;

    let resp = api.exchange_info(params).await.map_err(|e| {
        ParseSnafu {
            message: format!("exchange_info request failed: {e}"),
        }
        .build()
    })?;

    let info = resp.data().await.map_err(|e| {
        ParseSnafu {
            message: format!("exchange_info parse failed: {e}"),
        }
        .build()
    })?;

    let query_upper = query.to_uppercase();
    let symbols = info
        .symbols
        .unwrap_or_default()
        .into_iter()
        .filter(|s| {
            let symbol = s.symbol.as_deref().unwrap_or_default();
            let status_match = s.status.as_deref() == Some("TRADING");
            let quote_match = s.quote_asset.as_deref() == Some("USDT");
            status_match && quote_match && symbol.to_uppercase().contains(&query_upper)
        })
        .filter_map(|s| s.symbol)
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
        self.fetch_core(store, instrument_id, start, end, &|_| {})
            .await
    }
}

/// Extract the open time (ms) from a SDK kline row.
fn extract_open_time(row: &[KlinesItemInner]) -> Option<i64> {
    match row.first()? {
        KlinesItemInner::Integer(ms) => Some(*ms),
        _ => None,
    }
}

/// Parse a SDK kline row into a `CandleRow`.
fn parse_sdk_kline(row: &[KlinesItemInner], instrument_id: &str) -> Option<CandleRow> {
    let open_time_ms = extract_open_time(row)?;
    let open = extract_f64(row, 1)?;
    let high = extract_f64(row, 2)?;
    let low = extract_f64(row, 3)?;
    let close = extract_f64(row, 4)?;
    let volume = extract_f64(row, 5)?;
    let trade_count = extract_i64(row, 8).unwrap_or(0);

    Some(CandleRow {
        ts: DateTime::from_timestamp_millis(open_time_ms).unwrap_or(DateTime::<Utc>::MIN_UTC),
        instrument_id: instrument_id.to_string(),
        interval: "1m".to_string(),
        open,
        high,
        low,
        close,
        volume,
        trade_count: i32::try_from(trade_count).unwrap_or(i32::MAX),
    })
}

/// Extract an f64 from a kline row (SDK returns strings for decimal values).
fn extract_f64(row: &[KlinesItemInner], idx: usize) -> Option<f64> {
    match row.get(idx)? {
        KlinesItemInner::String(s) => s.parse().ok(),
        #[allow(clippy::cast_precision_loss)]
        KlinesItemInner::Integer(n) => Some(*n as f64),
        KlinesItemInner::Other(v) => v.as_f64(),
    }
}

/// Extract an i64 from a kline row.
fn extract_i64(row: &[KlinesItemInner], idx: usize) -> Option<i64> {
    match row.get(idx)? {
        KlinesItemInner::Integer(n) => Some(*n),
        KlinesItemInner::String(s) => s.parse().ok(),
        KlinesItemInner::Other(v) => v.as_i64(),
    }
}
