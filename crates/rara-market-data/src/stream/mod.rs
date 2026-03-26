//! Real-time market data streaming via WebSocket.

pub mod binance_ws;
pub mod reconnect;

pub use binance_ws::{BinanceWsClient, RawKline, WsError};
pub use reconnect::{ReconnectConfig, ReconnectingWsClient};
