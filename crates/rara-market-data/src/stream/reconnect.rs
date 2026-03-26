//! Auto-reconnecting WebSocket stream wrapper with exponential backoff.

use std::time::Duration;

use futures_util::StreamExt;
use tokio::sync::broadcast;

use super::binance_ws::{BinanceWsClient, RawKline, WsError};

/// Configuration for reconnection behavior.
#[derive(Debug, Clone)]
pub struct ReconnectConfig {
    /// Initial backoff delay after first failure.
    pub initial_delay: Duration,
    /// Maximum backoff delay cap.
    pub max_delay: Duration,
    /// Backoff multiplier applied after each consecutive failure.
    pub multiplier: f64,
    /// Number of consecutive failures before logging an alert.
    pub max_failures_before_alert: u32,
}

impl Default for ReconnectConfig {
    fn default() -> Self {
        Self {
            initial_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(60),
            multiplier: 2.0,
            max_failures_before_alert: 10,
        }
    }
}

/// Compute the next backoff delay given the current delay, multiplier, and cap.
///
/// Returns `min(current * multiplier, max_delay)`.
pub fn next_backoff(current: Duration, multiplier: f64, max_delay: Duration) -> Duration {
    let next_secs = (current.as_secs_f64() * multiplier).min(max_delay.as_secs_f64());
    Duration::from_secs_f64(next_secs)
}

/// A resilient WebSocket client that auto-reconnects on failure.
///
/// Wraps [`BinanceWsClient`] and transparently reconnects when the
/// underlying stream disconnects or errors. Klines are forwarded
/// through a broadcast channel so consumers are unaffected by reconnects.
pub struct ReconnectingWsClient {
    /// The underlying WebSocket client.
    pub client: BinanceWsClient,
    /// Reconnection configuration.
    pub config: ReconnectConfig,
    /// Broadcast sender for klines -- survives reconnects.
    sender: broadcast::Sender<RawKline>,
}

impl ReconnectingWsClient {
    /// Create a new reconnecting client with the given config.
    pub fn new(client: BinanceWsClient, config: ReconnectConfig) -> Self {
        let (sender, _) = broadcast::channel(1024);
        Self {
            client,
            config,
            sender,
        }
    }

    /// Subscribe to receive klines. The returned receiver survives reconnects.
    pub fn subscribe(&self) -> broadcast::Receiver<RawKline> {
        self.sender.subscribe()
    }

    /// Run the reconnecting stream loop for the given subscriptions.
    ///
    /// This method runs indefinitely, reconnecting with exponential backoff
    /// whenever the underlying WebSocket disconnects. It should be spawned
    /// as a background tokio task.
    ///
    /// Each element in `subscriptions` is a `(symbol, interval)` pair,
    /// e.g. `("BTCUSDT", "1m")`.
    pub async fn run(&self, subscriptions: Vec<(String, String)>) {
        let mut consecutive_failures: u32 = 0;
        let mut delay = self.config.initial_delay;

        loop {
            let sub_refs: Vec<(&str, &str)> = subscriptions
                .iter()
                .map(|(s, i)| (s.as_str(), i.as_str()))
                .collect();

            tracing::info!(
                subscriptions = ?subscriptions,
                "connecting to Binance WebSocket"
            );

            match self.client.subscribe_klines_multi(&sub_refs).await {
                Ok(mut stream) => {
                    // Successful connection resets backoff state
                    consecutive_failures = 0;
                    delay = self.config.initial_delay;
                    tracing::info!("WebSocket connected successfully");

                    while let Some(result) = stream.next().await {
                        match result {
                            Ok(kline) => {
                                // Ignore send errors (no active receivers)
                                let _ = self.sender.send(kline);
                            }
                            Err(WsError::StreamEnded) => {
                                tracing::warn!("WebSocket stream ended");
                                break;
                            }
                            Err(e) => {
                                tracing::error!(error = %e, "WebSocket stream error");
                                break;
                            }
                        }
                    }
                }
                Err(e) => {
                    tracing::error!(error = %e, "WebSocket connection failed");
                }
            }

            consecutive_failures += 1;

            if consecutive_failures >= self.config.max_failures_before_alert {
                tracing::error!(
                    consecutive_failures,
                    "WebSocket exceeded max failure threshold -- publishing alert"
                );
                // TODO(#77): publish alert event to event bus when available
            }

            tracing::warn!(
                attempt = consecutive_failures,
                delay_secs = delay.as_secs_f64(),
                "reconnecting after backoff"
            );

            tokio::time::sleep(delay).await;

            delay = next_backoff(delay, self.config.multiplier, self.config.max_delay);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_doubles_each_step() {
        let initial = Duration::from_secs(1);
        let max = Duration::from_secs(60);
        let multiplier = 2.0;

        let d1 = next_backoff(initial, multiplier, max);
        let d2 = next_backoff(d1, multiplier, max);
        let d3 = next_backoff(d2, multiplier, max);

        assert_eq!(d1, Duration::from_secs(2));
        assert_eq!(d2, Duration::from_secs(4));
        assert_eq!(d3, Duration::from_secs(8));
    }

    #[test]
    fn backoff_caps_at_max_delay() {
        let max = Duration::from_secs(60);
        let current = Duration::from_secs(32);
        let multiplier = 2.0;

        let d1 = next_backoff(current, multiplier, max);
        assert_eq!(d1, Duration::from_secs(60));

        // Further calls stay at cap
        let d2 = next_backoff(d1, multiplier, max);
        assert_eq!(d2, Duration::from_secs(60));
    }

    #[test]
    fn backoff_with_fractional_multiplier() {
        let current = Duration::from_secs(1);
        let max = Duration::from_secs(60);
        let multiplier = 1.5;

        let d1 = next_backoff(current, multiplier, max);
        // 1.0 * 1.5 = 1.5s
        assert_eq!(d1, Duration::from_secs_f64(1.5));

        let d2 = next_backoff(d1, multiplier, max);
        // 1.5 * 1.5 = 2.25s
        assert_eq!(d2, Duration::from_secs_f64(2.25));
    }

    #[test]
    fn default_config_has_expected_values() {
        let config = ReconnectConfig::default();
        assert_eq!(config.initial_delay, Duration::from_secs(1));
        assert_eq!(config.max_delay, Duration::from_secs(60));
        assert!((config.multiplier - 2.0).abs() < f64::EPSILON);
        assert_eq!(config.max_failures_before_alert, 10);
    }
}
