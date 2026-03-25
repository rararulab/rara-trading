//! Yahoo Finance historical data fetcher.
//!
//! Uses the public chart API (no authentication required).
//! Automatically selects 1m interval for recent data (<=7 days)
//! or 1d interval for older history.

use async_trait::async_trait;
use chrono::{DateTime, Days, NaiveDate, NaiveTime, Utc};
use serde::Deserialize;
use snafu::ResultExt;
use tracing::info;

use super::{HistoryFetcher, HttpSnafu, ParseSnafu, Result, StoreSnafu};
use crate::store::candle::CandleRow;
use crate::store::MarketStore;

/// Yahoo Finance chart API base URL.
const BASE_URL: &str = "https://query1.finance.yahoo.com/v8/finance/chart";

/// Fetches historical candles from Yahoo Finance.
pub struct YahooFetcher {
    /// HTTP client.
    pub client: reqwest::Client,
    /// Yahoo symbol, e.g. `"SPY"`.
    pub symbol: String,
}

impl YahooFetcher {
    /// Create a new fetcher for the given Yahoo Finance symbol.
    pub fn new(symbol: impl Into<String>) -> Self {
        Self {
            client: reqwest::Client::builder()
                .user_agent("Mozilla/5.0")
                .build()
                .expect("http client"),
            symbol: symbol.into(),
        }
    }

    /// Fetch candles for the given period and interval.
    async fn fetch_range(
        &self,
        period1: i64,
        period2: i64,
        interval: &str,
    ) -> Result<Vec<CandleRow>> {
        let url = format!(
            "{BASE_URL}/{}?period1={period1}&period2={period2}&interval={interval}",
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

        let body = resp.json::<YahooResponse>().await.context(HttpSnafu)?;

        let result = body.chart.result.first().ok_or_else(|| {
            ParseSnafu {
                message: "empty chart result".to_string(),
            }
            .build()
        })?;

        let timestamps = &result.timestamp;
        let quote = result.indicators.quote.first().ok_or_else(|| {
            ParseSnafu {
                message: "no quote data".to_string(),
            }
            .build()
        })?;

        let mut candles = Vec::with_capacity(timestamps.len());
        for (i, &ts) in timestamps.iter().enumerate() {
            let Some(open) = quote.open.get(i).copied().flatten() else {
                continue;
            };
            let Some(high) = quote.high.get(i).copied().flatten() else {
                continue;
            };
            let Some(low) = quote.low.get(i).copied().flatten() else {
                continue;
            };
            let Some(close) = quote.close.get(i).copied().flatten() else {
                continue;
            };
            let volume = quote.volume.get(i).copied().flatten().unwrap_or(0.0);

            candles.push(CandleRow {
                ts: DateTime::from_timestamp(ts, 0).unwrap_or(DateTime::<Utc>::MIN_UTC),
                instrument_id: String::new(),
                interval: String::new(),
                open,
                high,
                low,
                close,
                volume,
                trade_count: 0,
            });
        }

        Ok(candles)
    }
}

#[async_trait]
impl HistoryFetcher for YahooFetcher {
    async fn fetch_and_store(
        &self,
        store: &MarketStore,
        instrument_id: &str,
        start: NaiveDate,
        end: NaiveDate,
    ) -> Result<usize> {
        let period1 = start.and_time(NaiveTime::MIN).and_utc().timestamp();
        let period2 = end
            .checked_add_days(Days::new(1))
            .expect("date overflow")
            .and_time(NaiveTime::MIN)
            .and_utc()
            .timestamp();

        let now = Utc::now().naive_utc().date();
        let days_back = (now - start).num_days();
        let range_days = (end - start).num_days() + 1;
        let interval = if days_back <= 7 && range_days <= 7 {
            "1m"
        } else {
            "1d"
        };

        let mut candles = self.fetch_range(period1, period2, interval).await?;

        for c in &mut candles {
            c.instrument_id = instrument_id.to_string();
            c.interval = interval.to_string();
        }

        let count = store.insert_candles(&candles).await.context(StoreSnafu)?;
        info!(candles = count, interval, "yahoo: ingested");

        Ok(usize::try_from(count).expect("candle count fits in usize"))
    }
}

#[derive(Deserialize)]
struct YahooResponse {
    chart: YahooChart,
}

#[derive(Deserialize)]
struct YahooChart {
    result: Vec<YahooResult>,
}

#[derive(Deserialize)]
struct YahooResult {
    timestamp: Vec<i64>,
    indicators: YahooIndicators,
}

#[derive(Deserialize)]
struct YahooIndicators {
    quote: Vec<YahooQuote>,
}

#[derive(Deserialize)]
struct YahooQuote {
    open: Vec<Option<f64>>,
    high: Vec<Option<f64>>,
    low: Vec<Option<f64>>,
    close: Vec<Option<f64>>,
    volume: Vec<Option<f64>>,
}
