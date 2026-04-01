//! Binance WebSocket kline stream client.

use std::pin::Pin;

use futures_util::{Stream, StreamExt};
use serde::{Deserialize, Serialize};
use snafu::{ResultExt, Snafu};
use tokio_tungstenite::tungstenite::Message;

/// Errors from WebSocket streaming operations.
#[derive(Debug, Snafu)]
#[snafu(visibility(pub))]
pub enum WsError {
    /// WebSocket connection failed.
    #[snafu(display("WebSocket connection failed: {source}"))]
    Connection {
        /// The underlying tungstenite error (boxed to keep `WsError` small).
        source: Box<tokio_tungstenite::tungstenite::Error>,
    },

    /// Failed to parse kline message.
    #[snafu(display("failed to parse kline message: {source}"))]
    Parse {
        /// The underlying JSON parse error.
        source: serde_json::Error,
    },

    /// WebSocket stream ended unexpectedly.
    #[snafu(display("WebSocket stream ended unexpectedly"))]
    StreamEnded,
}

/// Result type for WebSocket operations.
pub type Result<T> = std::result::Result<T, WsError>;

/// Raw kline data received from a Binance WebSocket stream.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawKline {
    /// Symbol (e.g., "BTCUSDT").
    pub symbol:      String,
    /// Kline open time in milliseconds since epoch.
    pub open_time:   i64,
    /// Kline close time in milliseconds since epoch.
    pub close_time:  i64,
    /// Interval string (e.g., "1m", "1h").
    pub interval:    String,
    /// Open price.
    pub open:        f64,
    /// High price.
    pub high:        f64,
    /// Low price.
    pub low:         f64,
    /// Close price.
    pub close:       f64,
    /// Base asset volume.
    pub volume:      f64,
    /// Number of trades in this kline.
    pub trade_count: i32,
    /// Whether this kline is closed (final).
    pub is_closed:   bool,
}

/// Binance kline event wrapper matching the raw JSON structure.
#[derive(Debug, Deserialize)]
struct BinanceKlineEvent {
    #[serde(rename = "s")]
    symbol: String,
    #[serde(rename = "k")]
    kline:  BinanceKlineData,
}

/// Inner kline data fields from the Binance stream.
#[derive(Debug, Deserialize)]
struct BinanceKlineData {
    #[serde(rename = "t")]
    open_time:   i64,
    #[serde(rename = "T")]
    close_time:  i64,
    #[serde(rename = "i")]
    interval:    String,
    #[serde(rename = "o")]
    open:        String,
    #[serde(rename = "h")]
    high:        String,
    #[serde(rename = "l")]
    low:         String,
    #[serde(rename = "c")]
    close:       String,
    #[serde(rename = "v")]
    volume:      String,
    #[serde(rename = "n")]
    trade_count: i32,
    #[serde(rename = "x")]
    is_closed:   bool,
}

/// Combined stream wrapper used by Binance multi-symbol endpoints.
#[derive(Debug, Deserialize)]
struct CombinedStreamMessage {
    data: BinanceKlineEvent,
}

/// Binance WebSocket client for subscribing to real-time kline streams.
pub struct BinanceWsClient {
    /// Base WebSocket URL (without path).
    pub base_url: String,
}

impl Default for BinanceWsClient {
    fn default() -> Self {
        Self {
            base_url: "wss://stream.binance.com:9443".to_string(),
        }
    }
}

impl BinanceWsClient {
    /// Create a new client with the default Binance WebSocket URL.
    pub fn new() -> Self { Self::default() }

    /// Create a client with a custom WebSocket URL (useful for testing or
    /// proxies).
    pub fn with_url(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
        }
    }

    /// Subscribe to a single symbol's kline stream.
    ///
    /// Returns a `Stream` that yields `RawKline` items as they arrive.
    pub async fn subscribe_klines(
        &self,
        symbol: &str,
        interval: &str,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<RawKline>> + Send>>> {
        let symbol_lower = symbol.to_lowercase();
        let url = format!("{}/ws/{}@kline_{}", self.base_url, symbol_lower, interval);

        tracing::info!(%symbol, %interval, %url, "connecting to Binance kline stream");

        let (ws_stream, _) =
            tokio_tungstenite::connect_async(&url)
                .await
                .map_err(|e| WsError::Connection {
                    source: Box::new(e),
                })?;

        tracing::info!(%symbol, %interval, "WebSocket connected");

        let (_, read) = ws_stream.split();

        let stream = read.filter_map(|msg| {
            let result = match msg {
                Ok(Message::Text(text)) => Some(parse_kline_message(&text)),
                Ok(Message::Ping(_)) => {
                    tracing::trace!("received ping");
                    None
                }
                Ok(Message::Close(_)) => {
                    tracing::warn!("WebSocket closed by server");
                    Some(Err(WsError::StreamEnded))
                }
                Err(e) => Some(Err(WsError::Connection {
                    source: Box::new(e),
                })),
                _ => None,
            };
            std::future::ready(result)
        });

        Ok(Box::pin(stream))
    }

    /// Subscribe to multiple symbol kline streams via a combined connection.
    ///
    /// Each subscription is a `(symbol, interval)` pair. Binance multiplexes
    /// all streams over a single WebSocket connection.
    pub async fn subscribe_klines_multi(
        &self,
        subscriptions: &[(&str, &str)],
    ) -> Result<Pin<Box<dyn Stream<Item = Result<RawKline>> + Send>>> {
        let streams_param = subscriptions
            .iter()
            .map(|(symbol, interval)| format!("{}@kline_{}", symbol.to_lowercase(), interval))
            .collect::<Vec<_>>()
            .join("/");

        let url = format!("{}/stream?streams={}", self.base_url, streams_param);

        tracing::info!(?subscriptions, %url, "connecting to combined kline stream");

        let (ws_stream, _) =
            tokio_tungstenite::connect_async(&url)
                .await
                .map_err(|e| WsError::Connection {
                    source: Box::new(e),
                })?;

        tracing::info!("combined WebSocket connected");

        let (_, read) = ws_stream.split();

        // Combined stream wraps each event in {"stream":"...","data":{...}}
        let stream = read.filter_map(|msg| {
            let result = match msg {
                Ok(Message::Text(text)) => Some(parse_combined_message(&text)),
                Ok(Message::Close(_)) => {
                    tracing::warn!("WebSocket closed by server");
                    Some(Err(WsError::StreamEnded))
                }
                Err(e) => Some(Err(WsError::Connection {
                    source: Box::new(e),
                })),
                _ => None,
            };
            std::future::ready(result)
        });

        Ok(Box::pin(stream))
    }
}

/// Parse a single-stream kline message from Binance.
fn parse_kline_message(text: &str) -> Result<RawKline> {
    let event: BinanceKlineEvent = serde_json::from_str(text).context(ParseSnafu)?;
    Ok(kline_event_to_raw(event))
}

/// Parse a combined-stream kline message from Binance.
fn parse_combined_message(text: &str) -> Result<RawKline> {
    let msg: CombinedStreamMessage = serde_json::from_str(text).context(ParseSnafu)?;
    Ok(kline_event_to_raw(msg.data))
}

/// Convert the Binance-specific event structure into our domain `RawKline`.
///
/// Binance sends numeric values as strings to preserve decimal precision;
/// we parse them into `f64` here, defaulting to `0.0` on parse failure.
fn kline_event_to_raw(event: BinanceKlineEvent) -> RawKline {
    let k = event.kline;
    RawKline {
        symbol:      event.symbol,
        open_time:   k.open_time,
        close_time:  k.close_time,
        interval:    k.interval,
        open:        k.open.parse().unwrap_or(0.0),
        high:        k.high.parse().unwrap_or(0.0),
        low:         k.low.parse().unwrap_or(0.0),
        close:       k.close.parse().unwrap_or(0.0),
        volume:      k.volume.parse().unwrap_or(0.0),
        trade_count: k.trade_count,
        is_closed:   k.is_closed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_single_kline_extracts_all_fields() {
        let json = r#"{"e":"kline","E":1234567890,"s":"BTCUSDT","k":{"t":1234567800000,"T":1234567859999,"i":"1m","o":"67500.00","h":"67550.00","l":"67480.00","c":"67520.00","v":"1.234","n":42,"x":true}}"#;
        let kline = parse_kline_message(json).expect("should parse valid kline");

        assert_eq!(kline.symbol, "BTCUSDT");
        assert_eq!(kline.interval, "1m");
        assert_eq!(kline.open_time, 1_234_567_800_000);
        assert_eq!(kline.close_time, 1_234_567_859_999);
        assert!((kline.open - 67_500.0).abs() < f64::EPSILON);
        assert!((kline.high - 67_550.0).abs() < f64::EPSILON);
        assert!((kline.low - 67_480.0).abs() < f64::EPSILON);
        assert!((kline.close - 67_520.0).abs() < f64::EPSILON);
        assert!((kline.volume - 1.234).abs() < f64::EPSILON);
        assert_eq!(kline.trade_count, 42);
        assert!(kline.is_closed);
    }

    #[test]
    fn parse_combined_stream_unwraps_data_envelope() {
        let json = r#"{"stream":"btcusdt@kline_1m","data":{"e":"kline","E":1234567890,"s":"BTCUSDT","k":{"t":1234567800000,"T":1234567859999,"i":"1m","o":"67500.00","h":"67550.00","l":"67480.00","c":"67520.00","v":"1.234","n":42,"x":false}}}"#;
        let kline = parse_combined_message(json).expect("should parse combined message");

        assert_eq!(kline.symbol, "BTCUSDT");
        assert!(!kline.is_closed);
    }

    #[test]
    fn parse_kline_invalid_json_returns_error() {
        let result = parse_kline_message("not json");
        assert!(result.is_err());
    }

    #[test]
    fn parse_kline_handles_zero_volume_and_trades() {
        let json = r#"{"e":"kline","E":1,"s":"ETHUSDT","k":{"t":0,"T":0,"i":"5m","o":"0.0","h":"0.0","l":"0.0","c":"0.0","v":"0.0","n":0,"x":false}}"#;
        let kline = parse_kline_message(json).expect("should parse zero-value kline");

        assert_eq!(kline.symbol, "ETHUSDT");
        assert_eq!(kline.trade_count, 0);
        assert!((kline.volume).abs() < f64::EPSILON);
    }
}
