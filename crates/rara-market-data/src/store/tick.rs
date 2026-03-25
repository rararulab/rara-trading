//! Tick CRUD operations against `TimescaleDB`.

use chrono::{DateTime, Utc};
use snafu::ResultExt;
use tracing::debug;

use super::{DatabaseSnafu, MarketStore, Result};

/// A single tick/trade row.
#[derive(Debug, Clone)]
pub struct TickRow {
    /// Trade timestamp (UTC).
    pub ts: DateTime<Utc>,
    /// Instrument identifier.
    pub instrument_id: String,
    /// Trade price.
    pub price: f64,
    /// Trade amount.
    pub amount: f64,
    /// Trade side: 0 = buy, 1 = sell.
    pub side: i16,
}

impl MarketStore {
    /// Batch insert ticks.
    pub async fn insert_ticks(&self, ticks: &[TickRow]) -> Result<u64> {
        if ticks.is_empty() {
            return Ok(0);
        }

        let ts: Vec<DateTime<Utc>> = ticks.iter().map(|t| t.ts).collect();
        let instrument_ids: Vec<&str> = ticks.iter().map(|t| t.instrument_id.as_str()).collect();
        let prices: Vec<f64> = ticks.iter().map(|t| t.price).collect();
        let amounts: Vec<f64> = ticks.iter().map(|t| t.amount).collect();
        let sides: Vec<i16> = ticks.iter().map(|t| t.side).collect();

        let result = sqlx::query(
            "INSERT INTO ticks (ts, instrument_id, price, amount, side)
             SELECT * FROM UNNEST($1::timestamptz[], $2::text[], $3::float8[], $4::float8[], $5::int2[])",
        )
        .bind(&ts)
        .bind(&instrument_ids)
        .bind(&prices)
        .bind(&amounts)
        .bind(&sides)
        .execute(&self.pool)
        .await
        .context(DatabaseSnafu)?;

        debug!(rows = result.rows_affected(), "inserted ticks");
        Ok(result.rows_affected())
    }
}
