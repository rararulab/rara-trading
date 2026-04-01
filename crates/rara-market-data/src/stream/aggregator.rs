//! Multi-timeframe candle aggregation from 1m kline stream.
//!
//! Consumes closed 1-minute klines and emits aggregated candles (5m, 15m, 1h)
//! via a `broadcast::Sender`, allowing multiple downstream consumers.

use std::collections::HashMap;

use chrono::{DateTime, Timelike, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

use super::binance_ws::RawKline;

/// An aggregated candle ready for consumption.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AggregatedCandle {
    /// Symbol (e.g., "BTCUSDT").
    pub symbol:      String,
    /// Candle open timestamp (aligned to interval boundary).
    pub ts:          DateTime<Utc>,
    /// Timeframe interval string (e.g., "5m", "1h").
    pub interval:    String,
    /// Open price (first candle's open).
    pub open:        f64,
    /// Highest price across all constituent candles.
    pub high:        f64,
    /// Lowest price across all constituent candles.
    pub low:         f64,
    /// Close price (last candle's close).
    pub close:       f64,
    /// Total volume summed across all constituent candles.
    pub volume:      f64,
    /// Total trade count summed across all constituent candles.
    pub trade_count: i32,
}

/// Tracks partial candle state during aggregation.
struct PartialCandle {
    /// Aligned open timestamp for this bucket.
    open_ts:     DateTime<Utc>,
    open:        f64,
    high:        f64,
    low:         f64,
    close:       f64,
    volume:      f64,
    trade_count: i32,
    /// Number of 1m candles absorbed so far.
    count:       u32,
}

impl PartialCandle {
    const fn new(kline: &RawKline, open_ts: DateTime<Utc>) -> Self {
        Self {
            open_ts,
            open: kline.open,
            high: kline.high,
            low: kline.low,
            close: kline.close,
            volume: kline.volume,
            trade_count: kline.trade_count,
            count: 1,
        }
    }

    fn update(&mut self, kline: &RawKline) {
        self.high = self.high.max(kline.high);
        self.low = self.low.min(kline.low);
        self.close = kline.close;
        self.volume += kline.volume;
        self.trade_count += kline.trade_count;
        self.count += 1;
    }

    fn into_candle(self, symbol: &str, interval: &str) -> AggregatedCandle {
        AggregatedCandle {
            symbol:      symbol.to_string(),
            ts:          self.open_ts,
            interval:    interval.to_string(),
            open:        self.open,
            high:        self.high,
            low:         self.low,
            close:       self.close,
            volume:      self.volume,
            trade_count: self.trade_count,
        }
    }
}

/// Key for tracking partial candles: (symbol, interval).
type AggKey = (String, String);

/// Candle aggregator that consumes 1m klines and emits multi-timeframe candles.
///
/// Each incoming closed 1-minute kline is bucketed into configured target
/// intervals. When a bucket is full (i.e., the expected number of 1m candles
/// has been absorbed) or a new bucket starts, the completed candle is emitted
/// via a `broadcast::Sender`.
pub struct CandleAggregator {
    /// Target intervals to aggregate into: (`interval_name`, minutes).
    intervals: Vec<(String, u32)>,
    /// Partial candle buffers keyed by (symbol, interval).
    buffers:   HashMap<AggKey, PartialCandle>,
    /// Broadcast sender for completed candles.
    sender:    broadcast::Sender<AggregatedCandle>,
}

impl CandleAggregator {
    /// Create a new aggregator for the given target intervals.
    ///
    /// Returns the aggregator and a receiver for completed candles.
    pub fn new(intervals: &[(String, u32)]) -> (Self, broadcast::Receiver<AggregatedCandle>) {
        let (sender, receiver) = broadcast::channel(256);
        let aggregator = Self {
            intervals: intervals.to_vec(),
            buffers: HashMap::new(),
            sender,
        };
        (aggregator, receiver)
    }

    /// Create with default intervals: 5m, 15m, 1h.
    pub fn with_defaults() -> (Self, broadcast::Receiver<AggregatedCandle>) {
        let intervals = vec![
            ("5m".to_string(), 5),
            ("15m".to_string(), 15),
            ("1h".to_string(), 60),
        ];
        Self::new(&intervals)
    }

    /// Subscribe to receive completed candles.
    pub fn subscribe(&self) -> broadcast::Receiver<AggregatedCandle> { self.sender.subscribe() }

    /// Process a closed 1m kline, potentially emitting aggregated candles.
    ///
    /// Non-closed klines and non-1m intervals are silently ignored.
    pub fn process_kline(&mut self, kline: &RawKline) {
        // Only process closed 1m candles
        if !kline.is_closed || kline.interval != "1m" {
            return;
        }

        let kline_ts = DateTime::<Utc>::from_timestamp_millis(kline.open_time)
            .expect("kline open_time should be a valid millisecond timestamp");

        for (interval_name, interval_minutes) in &self.intervals {
            let key = (kline.symbol.clone(), interval_name.clone());
            let bucket_ts = align_timestamp(kline_ts, *interval_minutes);

            match self.buffers.get_mut(&key) {
                Some(partial) if partial.open_ts == bucket_ts => {
                    // Same bucket -- update partial candle
                    partial.update(kline);

                    // Emit if bucket is complete
                    if partial.count >= *interval_minutes {
                        let candle = self
                            .buffers
                            .remove(&key)
                            .expect("key was just matched")
                            .into_candle(&kline.symbol, interval_name);
                        let _ = self.sender.send(candle);
                    }
                }
                Some(_) => {
                    // New bucket started -- emit the old partial, start fresh
                    let old = self
                        .buffers
                        .remove(&key)
                        .expect("key was just matched")
                        .into_candle(&kline.symbol, interval_name);
                    let _ = self.sender.send(old);

                    self.buffers
                        .insert(key, PartialCandle::new(kline, bucket_ts));
                }
                None => {
                    // First candle for this bucket
                    self.buffers
                        .insert(key, PartialCandle::new(kline, bucket_ts));
                }
            }
        }
    }
}

/// Align a timestamp to the start of its interval bucket.
///
/// For sub-hour intervals (e.g., 5m, 15m), rounds the minute component
/// down to the nearest multiple. For 60m, aligns to the hour boundary.
fn align_timestamp(ts: DateTime<Utc>, interval_minutes: u32) -> DateTime<Utc> {
    if interval_minutes >= 60 {
        // Align to hour boundary
        ts.with_minute(0)
            .and_then(|t| t.with_second(0))
            .and_then(|t| t.with_nanosecond(0))
            .expect("zeroing minute/second/nanosecond should always succeed")
    } else {
        let minutes = ts.minute();
        let aligned_minute = (minutes / interval_minutes) * interval_minutes;
        ts.with_minute(aligned_minute)
            .and_then(|t| t.with_second(0))
            .and_then(|t| t.with_nanosecond(0))
            .expect("aligning minute/second/nanosecond should always succeed")
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    /// Helper to create a closed 1m `RawKline` at a given minute offset.
    fn make_kline(
        symbol: &str,
        hour: u32,
        minute: u32,
        ohlcv: (f64, f64, f64, f64, f64, i32),
    ) -> RawKline {
        let ts = Utc.with_ymd_and_hms(2025, 1, 15, hour, minute, 0).unwrap();
        RawKline {
            symbol:      symbol.to_string(),
            open_time:   ts.timestamp_millis(),
            close_time:  ts.timestamp_millis() + 59_999,
            interval:    "1m".to_string(),
            open:        ohlcv.0,
            high:        ohlcv.1,
            low:         ohlcv.2,
            close:       ohlcv.3,
            volume:      ohlcv.4,
            trade_count: ohlcv.5,
            is_closed:   true,
        }
    }

    #[test]
    fn align_5m_rounds_down_to_nearest_boundary() {
        let ts = Utc.with_ymd_and_hms(2025, 1, 15, 10, 7, 30).unwrap();
        let aligned = align_timestamp(ts, 5);
        assert_eq!(aligned.minute(), 5);
        assert_eq!(aligned.second(), 0);
    }

    #[test]
    fn align_15m_rounds_down() {
        let ts = Utc.with_ymd_and_hms(2025, 1, 15, 10, 22, 0).unwrap();
        let aligned = align_timestamp(ts, 15);
        assert_eq!(aligned.minute(), 15);
    }

    #[test]
    fn align_60m_zeros_minutes() {
        let ts = Utc.with_ymd_and_hms(2025, 1, 15, 10, 45, 0).unwrap();
        let aligned = align_timestamp(ts, 60);
        assert_eq!(aligned.minute(), 0);
        assert_eq!(aligned.hour(), 10);
    }

    #[test]
    fn five_1m_candles_produce_one_5m_candle_with_correct_ohlcv() {
        let intervals = vec![("5m".to_string(), 5u32)];
        let (mut agg, mut rx) = CandleAggregator::new(&intervals);

        // Minute 0: open=100, high=105, low=99, close=102, vol=10, trades=5
        agg.process_kline(&make_kline(
            "BTCUSDT",
            10,
            0,
            (100.0, 105.0, 99.0, 102.0, 10.0, 5),
        ));
        // Minute 1: high spike
        agg.process_kline(&make_kline(
            "BTCUSDT",
            10,
            1,
            (102.0, 110.0, 101.0, 108.0, 20.0, 8),
        ));
        // Minute 2: low dip
        agg.process_kline(&make_kline(
            "BTCUSDT",
            10,
            2,
            (108.0, 109.0, 95.0, 97.0, 15.0, 3),
        ));
        // Minute 3
        agg.process_kline(&make_kline(
            "BTCUSDT",
            10,
            3,
            (97.0, 100.0, 96.0, 99.0, 12.0, 6),
        ));

        // After 4 candles, nothing should be emitted yet
        assert!(rx.try_recv().is_err());

        // Minute 4: completes the 5m bucket
        agg.process_kline(&make_kline(
            "BTCUSDT",
            10,
            4,
            (99.0, 103.0, 98.0, 101.0, 18.0, 7),
        ));

        let candle = rx.try_recv().expect("should emit 5m candle");
        assert_eq!(candle.symbol, "BTCUSDT");
        assert_eq!(candle.interval, "5m");
        // open = first candle's open
        assert!((candle.open - 100.0).abs() < f64::EPSILON);
        // high = max across all (110.0 from minute 1)
        assert!((candle.high - 110.0).abs() < f64::EPSILON);
        // low = min across all (95.0 from minute 2)
        assert!((candle.low - 95.0).abs() < f64::EPSILON);
        // close = last candle's close
        assert!((candle.close - 101.0).abs() < f64::EPSILON);
        // volume = sum
        assert!((candle.volume - 75.0).abs() < f64::EPSILON);
        // trade_count = sum
        assert_eq!(candle.trade_count, 29);
    }

    #[test]
    fn partial_bucket_emitted_when_new_bucket_starts() {
        let intervals = vec![("5m".to_string(), 5u32)];
        let (mut agg, mut rx) = CandleAggregator::new(&intervals);

        // Feed 3 of 5 candles in the 10:00-10:04 bucket
        agg.process_kline(&make_kline(
            "BTCUSDT",
            10,
            0,
            (100.0, 105.0, 99.0, 102.0, 10.0, 5),
        ));
        agg.process_kline(&make_kline(
            "BTCUSDT",
            10,
            1,
            (102.0, 106.0, 101.0, 104.0, 8.0, 3),
        ));
        agg.process_kline(&make_kline(
            "BTCUSDT",
            10,
            2,
            (104.0, 107.0, 103.0, 105.0, 12.0, 4),
        ));

        // No emission yet
        assert!(rx.try_recv().is_err());

        // Jump to next bucket (10:05) -- should flush the partial
        agg.process_kline(&make_kline(
            "BTCUSDT",
            10,
            5,
            (105.0, 108.0, 104.0, 107.0, 15.0, 6),
        ));

        let candle = rx
            .try_recv()
            .expect("partial bucket should be emitted on new bucket");
        assert_eq!(candle.interval, "5m");
        // Partial had 3 candles: open from first, close from third
        assert!((candle.open - 100.0).abs() < f64::EPSILON);
        assert!((candle.close - 105.0).abs() < f64::EPSILON);
        assert!((candle.volume - 30.0).abs() < f64::EPSILON);
        assert_eq!(candle.trade_count, 12);
    }

    #[test]
    fn multiple_symbols_tracked_independently() {
        let intervals = vec![("5m".to_string(), 5u32)];
        let (mut agg, mut rx) = CandleAggregator::new(&intervals);

        // Feed 5 candles for BTC and only 3 for ETH
        for m in 0..5 {
            agg.process_kline(&make_kline(
                "BTCUSDT",
                10,
                m,
                (100.0, 105.0, 99.0, 102.0, 10.0, 1),
            ));
        }
        for m in 0..3 {
            agg.process_kline(&make_kline(
                "ETHUSDT",
                10,
                m,
                (3000.0, 3100.0, 2900.0, 3050.0, 5.0, 2),
            ));
        }

        // BTC should have emitted, ETH should not
        let btc_candle = rx.try_recv().expect("BTC 5m should be emitted");
        assert_eq!(btc_candle.symbol, "BTCUSDT");

        // ETH has no emission yet (only 3 of 5)
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn non_closed_klines_are_ignored() {
        let intervals = vec![("5m".to_string(), 5u32)];
        let (mut agg, mut rx) = CandleAggregator::new(&intervals);

        let mut kline = make_kline("BTCUSDT", 10, 0, (100.0, 105.0, 99.0, 102.0, 10.0, 5));
        kline.is_closed = false;
        agg.process_kline(&kline);

        // Buffer should be empty -- nothing was processed
        assert!(agg.buffers.is_empty());
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn non_1m_intervals_are_ignored() {
        let intervals = vec![("5m".to_string(), 5u32)];
        let (mut agg, mut rx) = CandleAggregator::new(&intervals);

        let mut kline = make_kline("BTCUSDT", 10, 0, (100.0, 105.0, 99.0, 102.0, 10.0, 5));
        kline.interval = "5m".to_string();
        agg.process_kline(&kline);

        assert!(agg.buffers.is_empty());
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn multiple_intervals_emit_independently() {
        let intervals = vec![("5m".to_string(), 5u32), ("15m".to_string(), 15u32)];
        let (mut agg, mut rx) = CandleAggregator::new(&intervals);

        // Feed 5 candles (minute 0-4) -- should complete 5m but not 15m
        for m in 0..5 {
            agg.process_kline(&make_kline(
                "BTCUSDT",
                10,
                m,
                (100.0, 105.0, 99.0, 102.0, 10.0, 1),
            ));
        }

        let candle = rx.try_recv().expect("5m candle should emit");
        assert_eq!(candle.interval, "5m");

        // 15m should not have emitted
        assert!(rx.try_recv().is_err());

        // Feed remaining 10 candles to complete the 15m bucket
        for m in 5..15 {
            agg.process_kline(&make_kline(
                "BTCUSDT",
                10,
                m,
                (102.0, 106.0, 98.0, 103.0, 8.0, 2),
            ));
        }

        // Should have emitted two more 5m candles (10:05-10:09, 10:10-10:14) and one
        // 15m
        let mut intervals_seen = Vec::new();
        while let Ok(c) = rx.try_recv() {
            intervals_seen.push(c.interval.clone());
        }
        assert!(intervals_seen.contains(&"5m".to_string()));
        assert!(intervals_seen.contains(&"15m".to_string()));
    }

    #[test]
    fn subscribe_creates_independent_receiver() {
        let intervals = vec![("5m".to_string(), 5u32)];
        let (mut agg, mut rx1) = CandleAggregator::new(&intervals);
        let mut rx2 = agg.subscribe();

        for m in 0..5 {
            agg.process_kline(&make_kline(
                "BTCUSDT",
                10,
                m,
                (100.0, 105.0, 99.0, 102.0, 10.0, 1),
            ));
        }

        // Both receivers should get the candle
        assert!(rx1.try_recv().is_ok());
        assert!(rx2.try_recv().is_ok());
    }
}
