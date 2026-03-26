//! Market data storage and fetching backed by `TimescaleDB`.
//!
//! Provides a `TimescaleDB`-backed store for persistence and querying
//! of candles and tick data.

pub mod fetcher;
pub mod store;
pub mod stream;
