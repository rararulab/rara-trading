//! Real-time market data streaming via WebSocket.

pub mod aggregator;
pub mod binance_ws;
pub mod persister;
pub mod reconnect;

pub use aggregator::{AggregatedCandle, CandleAggregator};
pub use binance_ws::{BinanceWsClient, RawKline, WsError};
pub use persister::spawn_candle_persister;
pub use reconnect::{ReconnectConfig, ReconnectingWsClient};
