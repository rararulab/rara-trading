//! Barter [`AlgoStrategy`] adapter that bridges any [`StrategyHandle`] with the barter engine.
//!
//! Receives 1-minute candle data from [`CandleInstrumentData`], aggregates to the target
//! [`Timeframe`], and delegates signal generation to a [`StrategyHandle`] (runtime-agnostic).

use std::cell::RefCell;

use barter::engine::Engine;
use barter::engine::state::EngineState;
use barter::engine::state::global::DefaultGlobalData;
use barter::engine::state::instrument::data::InstrumentDataState;
use barter::engine::state::instrument::filter::InstrumentFilter;
use barter::strategy::algo::AlgoStrategy;
use barter::strategy::close_positions::{ClosePositionsStrategy, close_open_positions_with_market_orders};
use barter::strategy::on_disconnect::OnDisconnectStrategy;
use barter::strategy::on_trading_disabled::OnTradingDisabled;
use barter_execution::order::id::{ClientOrderId, StrategyId};
use barter_execution::order::request::{OrderRequestCancel, OrderRequestOpen, RequestOpen};
use barter_execution::order::{OrderKey, OrderKind, TimeInForce};
use barter_instrument::Side;
use barter_instrument::asset::AssetIndex;
use barter_instrument::exchange::{ExchangeId, ExchangeIndex};
use barter_instrument::instrument::InstrumentIndex;
use rust_decimal::{Decimal, prelude::FromPrimitive};
use tracing::warn;

use rara_domain::timeframe::Timeframe;
use rara_strategy_api::{Candle as ApiCandle, Signal};

use crate::candle_instrument_data::CandleInstrumentData;
use crate::strategy_executor::StrategyHandle;

/// Maximum number of aggregated candles retained in history.
const MAX_AGGREGATED_HISTORY: usize = 500;

/// Engine state type alias using our candle-aware instrument data.
pub type BacktestEngineState = EngineState<DefaultGlobalData, CandleInstrumentData>;

/// Barter strategy adapter that delegates signal generation to any [`StrategyHandle`].
///
/// Aggregates 1-minute candles from [`CandleInstrumentData`] into the target [`Timeframe`],
/// then calls the handle's `on_candles()` when a new aggregated bar completes.
/// Uses [`RefCell`] for interior mutability because barter's [`AlgoStrategy`] requires `&self`.
pub struct BarterStrategy {
    /// Strategy identifier for order tagging.
    pub id: StrategyId,
    /// Mutable inner state behind `RefCell` (barter calls `generate_algo_orders` with `&self`).
    state: RefCell<StrategyInner>,
}

/// Mutable state for candle aggregation and strategy invocation.
struct StrategyInner {
    /// Strategy handle for calling `on_candles()`.
    handle: Box<dyn StrategyHandle>,
    /// Target timeframe for candle aggregation.
    timeframe: Timeframe,
    /// Buffer of 1m candles accumulating toward next aggregation boundary.
    buffer: Vec<ApiCandle>,
    /// Aggregated candle history passed to the WASM strategy.
    history: Vec<ApiCandle>,
    /// Number of candles already consumed from `CandleInstrumentData`.
    processed_count: usize,
    /// Most recent signal from the WASM strategy.
    current_signal: Option<Signal>,
}

impl BarterStrategy {
    /// Create a new strategy adapter.
    ///
    /// # Arguments
    /// * `handle` - Executable strategy handle (runtime-agnostic)
    /// * `timeframe` - Target timeframe for candle aggregation (e.g., 4h)
    pub fn new(handle: Box<dyn StrategyHandle>, timeframe: Timeframe) -> Self {
        Self {
            id: StrategyId::new("strategy"),
            state: RefCell::new(StrategyInner {
                handle,
                timeframe,
                buffer: Vec::new(),
                history: Vec::new(),
                processed_count: 0,
                current_signal: None,
            }),
        }
    }
}

/// Check whether the buffer of 1m candles should be aggregated into one target-timeframe bar.
///
/// For 1m timeframe, every candle passes through directly. For larger timeframes,
/// aggregation triggers when the next minute would cross a natural timeframe boundary.
fn should_aggregate(buffer: &[ApiCandle], timeframe: Timeframe) -> bool {
    if timeframe == Timeframe::Min1 {
        return true;
    }
    if buffer.is_empty() {
        return false;
    }
    let secs = timeframe.seconds();
    let last_ts = buffer.last().expect("buffer checked non-empty").timestamp;
    // Natural boundary: next minute would cross the timeframe boundary
    (last_ts + 60) % secs == 0
}

/// Aggregate a slice of 1m candles into a single OHLCV bar.
///
/// Panics if `candles` is empty.
fn aggregate_candles(candles: &[ApiCandle]) -> ApiCandle {
    let first = candles.first().expect("aggregate requires non-empty candles");
    let last = candles.last().expect("aggregate requires non-empty candles");
    ApiCandle {
        timestamp: first.timestamp,
        open: first.open,
        high: candles.iter().map(|c| c.high).fold(f64::NEG_INFINITY, f64::max),
        low: candles.iter().map(|c| c.low).fold(f64::INFINITY, f64::min),
        close: last.close,
        volume: candles.iter().map(|c| c.volume).sum(),
    }
}

impl AlgoStrategy for BarterStrategy {
    type State = BacktestEngineState;

    fn generate_algo_orders(
        &self,
        state: &Self::State,
    ) -> (
        impl IntoIterator<Item = OrderRequestCancel<ExchangeIndex, InstrumentIndex>>,
        impl IntoIterator<Item = OrderRequestOpen<ExchangeIndex, InstrumentIndex>>,
    ) {
        let mut inner = self.state.borrow_mut();
        let mut open_orders: Vec<OrderRequestOpen<ExchangeIndex, InstrumentIndex>> = Vec::new();

        // Process each instrument's candle data
        for (_name, instrument_state) in &state.instruments.0 {
            let candles = instrument_state.data.candle_history();
            let new_count = candles.len();

            if new_count <= inner.processed_count {
                continue;
            }

            // Process new candles since last check
            let new_candles = &candles[inner.processed_count..];
            for candle in new_candles {
                inner.buffer.push(candle.clone());

                if should_aggregate(&inner.buffer, inner.timeframe) {
                    let aggregated = aggregate_candles(&inner.buffer);
                    inner.buffer.clear();
                    inner.history.push(aggregated);

                    // Cap history to prevent unbounded growth
                    if inner.history.len() > MAX_AGGREGATED_HISTORY {
                        let excess = inner.history.len() - MAX_AGGREGATED_HISTORY;
                        inner.history.drain(..excess);
                    }

                    // Clone history to avoid conflicting borrows on inner
                    let history_snapshot = inner.history.clone();
                    match inner.handle.on_candles(&history_snapshot) {
                        Ok(signal) => inner.current_signal = Some(signal),
                        Err(err) => {
                            warn!(%err, "strategy on_candles failed");
                        }
                    }
                }
            }

            inner.processed_count = new_count;

            // Convert current signal to order requests
            if let Some(ref signal) = inner.current_signal {
                let price = instrument_state.data.price();

                match signal {
                    Signal::Entry { side, strength } => {
                        if let Some(price) = price {
                            let order_side = match side {
                                rara_strategy_api::Side::Long => Side::Buy,
                                rara_strategy_api::Side::Short => Side::Sell,
                            };
                            let quantity = Decimal::from_f64(*strength)
                                .unwrap_or(Decimal::ONE);

                            open_orders.push(OrderRequestOpen {
                                key: OrderKey {
                                    exchange: instrument_state.instrument.exchange,
                                    instrument: instrument_state.key,
                                    strategy: self.id.clone(),
                                    cid: ClientOrderId::random(),
                                },
                                state: RequestOpen {
                                    side: order_side,
                                    price,
                                    quantity,
                                    kind: OrderKind::Market,
                                    time_in_force: TimeInForce::ImmediateOrCancel,
                                },
                            });
                        }
                    }
                    Signal::Exit => {
                        // Close position by placing opposite-side market order
                        if let Some(position) = instrument_state.position.current.as_ref()
                            && let Some(price) = price
                        {
                            let exit_side = match position.side {
                                Side::Buy => Side::Sell,
                                Side::Sell => Side::Buy,
                            };
                            open_orders.push(OrderRequestOpen {
                                key: OrderKey {
                                    exchange: instrument_state.instrument.exchange,
                                    instrument: instrument_state.key,
                                    strategy: self.id.clone(),
                                    cid: ClientOrderId::random(),
                                },
                                state: RequestOpen {
                                    side: exit_side,
                                    price,
                                    quantity: position.quantity_abs,
                                    kind: OrderKind::Market,
                                    time_in_force: TimeInForce::ImmediateOrCancel,
                                },
                            });
                        }
                    }
                    Signal::Hold => {}
                }
            }
        }

        (std::iter::empty(), open_orders)
    }
}

impl ClosePositionsStrategy for BarterStrategy {
    type State = BacktestEngineState;

    fn close_positions_requests<'a>(
        &'a self,
        state: &'a Self::State,
        filter: &'a InstrumentFilter,
    ) -> (
        impl IntoIterator<Item = OrderRequestCancel<ExchangeIndex, InstrumentIndex>> + 'a,
        impl IntoIterator<Item = OrderRequestOpen<ExchangeIndex, InstrumentIndex>> + 'a,
    )
    where
        ExchangeIndex: 'a,
        AssetIndex: 'a,
        InstrumentIndex: 'a,
    {
        close_open_positions_with_market_orders(&self.id, state, filter, |_| {
            ClientOrderId::random()
        })
    }
}

impl<Clock, ExecutionTxs, Risk> OnDisconnectStrategy<Clock, BacktestEngineState, ExecutionTxs, Risk>
    for BarterStrategy
{
    type OnDisconnect = ();

    fn on_disconnect(
        _: &mut Engine<Clock, BacktestEngineState, ExecutionTxs, Self, Risk>,
        _: ExchangeId,
    ) -> Self::OnDisconnect {
    }
}

impl<Clock, ExecutionTxs, Risk> OnTradingDisabled<Clock, BacktestEngineState, ExecutionTxs, Risk>
    for BarterStrategy
{
    type OnTradingDisabled = ();

    fn on_trading_disabled(
        _: &mut Engine<Clock, BacktestEngineState, ExecutionTxs, Self, Risk>,
    ) -> Self::OnTradingDisabled {
    }
}
