//! Real-time market data streaming via WebSocket.

pub mod binance_ws;

pub use binance_ws::{BinanceWsClient, RawKline, WsError};
