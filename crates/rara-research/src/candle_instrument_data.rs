//! Custom barter [`InstrumentDataState`] that accumulates candle history.
//!
//! Unlike [`DefaultInstrumentMarketData`](barter::engine::state::instrument::data::DefaultInstrumentMarketData)
//! which ignores candle events, this implementation stores OHLCV candle history
//! so strategies can read historical candle data for technical analysis.

use barter::{
    Timed,
    engine::{
        Processor,
        state::{
            instrument::data::InstrumentDataState,
            order::in_flight_recorder::InFlightRequestRecorder,
        },
    },
};
use barter_data::{
    event::{DataKind, MarketEvent},
    subscription::book::OrderBookL1,
};
use barter_execution::{
    AccountEvent,
    order::request::{OrderRequestCancel, OrderRequestOpen},
};
use rara_strategy_api::Candle as ApiCandle;
use rust_decimal::{Decimal, prelude::FromPrimitive};
use serde::{Deserialize, Serialize};

/// Maximum number of candles to retain in history.
///
/// Bounded to prevent unbounded memory growth during long backtests.
const MAX_CANDLE_HISTORY: usize = 10_000;

/// Instrument data state that tracks candle history in addition to price.
///
/// Mirrors [`DefaultInstrumentMarketData`](barter::engine::state::instrument::data::DefaultInstrumentMarketData)
/// for L1 order book and last traded price tracking, but additionally
/// accumulates OHLCV candles from `DataKind::Candle` events for strategy
/// consumption.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CandleInstrumentData {
    /// Latest L1 order book snapshot.
    pub l1:                OrderBookL1,
    /// Last traded price with timestamp.
    pub last_traded_price: Option<Timed<Decimal>>,
    /// Accumulated candle history, oldest first.
    pub candles:           Vec<ApiCandle>,
}

impl CandleInstrumentData {
    /// Returns a slice of all accumulated candles, oldest first.
    pub fn candle_history(&self) -> &[ApiCandle] { &self.candles }
}

impl InstrumentDataState for CandleInstrumentData {
    type MarketEventKind = DataKind;

    fn price(&self) -> Option<Decimal> {
        self.l1
            .volume_weighed_mid_price()
            .or_else(|| self.last_traded_price.as_ref().map(|timed| timed.value))
    }
}

impl<InstrumentKey> Processor<&MarketEvent<InstrumentKey, DataKind>> for CandleInstrumentData {
    type Audit = ();

    fn process(&mut self, event: &MarketEvent<InstrumentKey, DataKind>) -> Self::Audit {
        match &event.kind {
            DataKind::Trade(trade) => {
                if self
                    .last_traded_price
                    .as_ref()
                    .is_none_or(|price| price.time < event.time_exchange)
                    && let Some(price) = Decimal::from_f64(trade.price)
                {
                    self.last_traded_price
                        .replace(Timed::new(price, event.time_exchange));
                }
            }
            DataKind::OrderBookL1(l1) => {
                if self.l1.last_update_time < event.time_exchange {
                    self.l1 = l1.clone();
                }
            }
            DataKind::Candle(candle) => {
                let api_candle = ApiCandle {
                    timestamp: event.time_exchange.timestamp(),
                    open:      candle.open,
                    high:      candle.high,
                    low:       candle.low,
                    close:     candle.close,
                    volume:    candle.volume,
                };

                self.candles.push(api_candle);

                // Evict oldest candles when history exceeds the cap.
                if self.candles.len() > MAX_CANDLE_HISTORY {
                    let excess = self.candles.len() - MAX_CANDLE_HISTORY;
                    self.candles.drain(..excess);
                }
            }
            _ => {}
        }
    }
}

impl<ExchangeKey, AssetKey, InstrumentKey>
    Processor<&AccountEvent<ExchangeKey, AssetKey, InstrumentKey>> for CandleInstrumentData
{
    type Audit = ();

    fn process(&mut self, _: &AccountEvent<ExchangeKey, AssetKey, InstrumentKey>) -> Self::Audit {}
}

impl<ExchangeKey, InstrumentKey> InFlightRequestRecorder<ExchangeKey, InstrumentKey>
    for CandleInstrumentData
{
    fn record_in_flight_cancel(&mut self, _: &OrderRequestCancel<ExchangeKey, InstrumentKey>) {}

    fn record_in_flight_open(&mut self, _: &OrderRequestOpen<ExchangeKey, InstrumentKey>) {}
}
