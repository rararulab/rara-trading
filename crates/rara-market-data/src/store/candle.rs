//! Candle CRUD operations against `TimescaleDB`.

use chrono::{DateTime, NaiveDate, NaiveTime, Utc};
use snafu::ResultExt;
use tracing::debug;

use super::{DatabaseSnafu, MarketStore, Result};

/// A single OHLCV candle row, used for both insert and query.
#[derive(Debug, Clone)]
pub struct CandleRow {
    /// Candle open timestamp (UTC).
    pub ts: DateTime<Utc>,
    /// Instrument identifier, e.g. `"binance-BTCUSDT"`.
    pub instrument_id: String,
    /// Candle interval, e.g. `"1m"`, `"1d"`.
    pub interval: String,
    /// Open price.
    pub open: f64,
    /// High price.
    pub high: f64,
    /// Low price.
    pub low: f64,
    /// Close price.
    pub close: f64,
    /// Volume.
    pub volume: f64,
    /// Number of trades.
    pub trade_count: i32,
}

impl MarketStore {
    /// Batch insert candles with ON CONFLICT DO NOTHING for idempotency.
    ///
    /// Returns the number of rows actually inserted (excluding conflicts).
    pub async fn insert_candles(&self, candles: &[CandleRow]) -> Result<u64> {
        if candles.is_empty() {
            return Ok(0);
        }

        let ts: Vec<DateTime<Utc>> = candles.iter().map(|c| c.ts).collect();
        let instrument_ids: Vec<&str> = candles.iter().map(|c| c.instrument_id.as_str()).collect();
        let intervals: Vec<&str> = candles.iter().map(|c| c.interval.as_str()).collect();
        let opens: Vec<f64> = candles.iter().map(|c| c.open).collect();
        let highs: Vec<f64> = candles.iter().map(|c| c.high).collect();
        let lows: Vec<f64> = candles.iter().map(|c| c.low).collect();
        let closes: Vec<f64> = candles.iter().map(|c| c.close).collect();
        let volumes: Vec<f64> = candles.iter().map(|c| c.volume).collect();
        let trade_counts: Vec<i32> = candles.iter().map(|c| c.trade_count).collect();

        let result = sqlx::query(
            "INSERT INTO candles (ts, instrument_id, interval, open, high, low, close, volume, trade_count)
             SELECT * FROM UNNEST($1::timestamptz[], $2::text[], $3::text[], $4::float8[], $5::float8[], $6::float8[], $7::float8[], $8::float8[], $9::int4[])
             ON CONFLICT (ts, instrument_id, interval) DO NOTHING",
        )
        .bind(&ts)
        .bind(&instrument_ids)
        .bind(&intervals)
        .bind(&opens)
        .bind(&highs)
        .bind(&lows)
        .bind(&closes)
        .bind(&volumes)
        .bind(&trade_counts)
        .execute(&self.pool)
        .await
        .context(DatabaseSnafu)?;

        debug!(rows = result.rows_affected(), "inserted candles");
        Ok(result.rows_affected())
    }

    /// Query candles for a given instrument and time range, ordered by time ascending.
    pub async fn query_candles(
        &self,
        instrument_id: &str,
        interval: &str,
        start: NaiveDate,
        end: NaiveDate,
    ) -> Result<Vec<CandleRow>> {
        let start_dt = start.and_time(NaiveTime::MIN).and_utc();
        let end_dt = end
            .succ_opt()
            .unwrap_or(end)
            .and_time(NaiveTime::MIN)
            .and_utc();

        let rows = sqlx::query_as::<_, CandleQueryRow>(
            "SELECT ts, instrument_id, interval, open, high, low, close, volume, trade_count
             FROM candles
             WHERE instrument_id = $1
               AND interval = $2
               AND ts >= $3
               AND ts < $4
             ORDER BY ts ASC",
        )
        .bind(instrument_id)
        .bind(interval)
        .bind(start_dt)
        .bind(end_dt)
        .fetch_all(&self.pool)
        .await
        .context(DatabaseSnafu)?;

        Ok(rows.into_iter().map(CandleRow::from).collect())
    }
}

/// Internal query result type that implements `sqlx::FromRow`.
#[derive(sqlx::FromRow)]
struct CandleQueryRow {
    ts: DateTime<Utc>,
    instrument_id: String,
    interval: String,
    open: f64,
    high: f64,
    low: f64,
    close: f64,
    volume: f64,
    trade_count: i32,
}

impl From<CandleQueryRow> for CandleRow {
    fn from(row: CandleQueryRow) -> Self {
        Self {
            ts: row.ts,
            instrument_id: row.instrument_id,
            interval: row.interval,
            open: row.open,
            high: row.high,
            low: row.low,
            close: row.close,
            volume: row.volume,
            trade_count: row.trade_count,
        }
    }
}
