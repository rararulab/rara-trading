//! Real-time market data streaming via WebSocket.

pub mod aggregator;
pub mod binance_ws;

pub use aggregator::{AggregatedCandle, CandleAggregator};
pub use binance_ws::{BinanceWsClient, RawKline, WsError};
