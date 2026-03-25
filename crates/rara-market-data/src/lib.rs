#![allow(unsafe_code)]
//! Dual-layer market data storage: hot binary (`.rara`) + cold Parquet.
//!
//! Provides zero-copy mmap reads for high-performance backtesting,
//! with moka-based caching and automatic hot-to-cold archival.

pub mod archiver;
pub mod cache;
pub mod file;
pub mod ingester;
pub mod record;
