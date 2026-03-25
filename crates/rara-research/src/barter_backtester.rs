//! Real backtester implementation using the barter-rs engine.
//!
//! Queries historical market data from `TimescaleDB`, runs it through barter's
//! engine with a compiled strategy, and extracts performance metrics into
//! our domain `BacktestResult`.

use std::fmt;
use std::sync::Arc;

use async_trait::async_trait;
use barter::backtest::market_data::MarketDataInMemory;
use barter::backtest::{BacktestArgsConstant, BacktestArgsDynamic, run_backtests};
use barter::engine::state::EngineState;
use barter::engine::state::global::DefaultGlobalData;
use barter::engine::state::trading::TradingState;
use barter::risk::DefaultRiskManager;
use barter::statistic::time::Daily;
use barter::system::config::ExecutionConfig;
use barter_data::event::{DataKind, MarketEvent};
use barter_data::streams::reconnect::Event as ReconnectEvent;
use barter_data::subscription::candle::Candle;
use barter_execution::balance::Balance;
use barter_execution::client::mock::MockExecutionConfig;
use barter_execution::UnindexedAccountSnapshot;
use barter_instrument::asset::name::AssetNameExchange;
use barter_instrument::asset::Asset;
use barter_instrument::exchange::ExchangeId;
use barter_instrument::index::IndexedInstruments;
use barter_instrument::instrument::{Instrument, InstrumentIndex};
use barter_instrument::Underlying;
use bon::Builder;
use chrono::NaiveDate;
use rust_decimal::Decimal;
use smol_str::SmolStr;

use rara_domain::research::BacktestResult;
use rara_domain::timeframe::Timeframe;
use rara_market_data::store::MarketStore;

use crate::backtester::{BacktestError, Backtester};
use crate::candle_instrument_data::CandleInstrumentData;
use crate::strategy_executor::StrategyExecutor;
use crate::barter_strategy::{BacktestEngineState, BarterStrategy};

/// Risk manager type parameterized over our engine state.
type BtRisk = DefaultRiskManager<BacktestEngineState>;

/// Real backtester powered by the barter-rs trading engine.
///
/// Queries historical market data from `TimescaleDB`, runs it through barter's
/// backtest infrastructure with a compiled strategy and mock execution,
/// and extracts performance metrics into our domain `BacktestResult`.
#[derive(Builder)]
pub struct BarterBacktester {
    /// `TimescaleDB` market data store.
    pub store: MarketStore,
    /// Initial capital for the simulated account.
    pub initial_capital: Decimal,
    /// Trading fees as a percentage (e.g., 0.1 for 0.1%).
    pub fees_percent: Decimal,
    /// Strategy executor for loading compiled artifacts into executable handles.
    pub executor: Arc<dyn StrategyExecutor>,
    /// Backtest window start date.
    pub backtest_start: NaiveDate,
    /// Backtest window end date.
    pub backtest_end: NaiveDate,
}

impl fmt::Debug for BarterBacktester {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BarterBacktester")
            .field("initial_capital", &self.initial_capital)
            .field("fees_percent", &self.fees_percent)
            .field("executor", &"<dyn StrategyExecutor>")
            .field("backtest_start", &self.backtest_start)
            .field("backtest_end", &self.backtest_end)
            .finish_non_exhaustive()
    }
}

/// Extract performance metrics from a barter `TradingSummary` into our domain `BacktestResult`.
///
/// Aggregates `PnL`, Sharpe ratio, max drawdown, and win rate across all instruments
/// in the trading summary, tagging the result with the given timeframe.
fn extract_metrics(
    trading_summary: &barter::statistic::summary::TradingSummary<Daily>,
    timeframe: Timeframe,
) -> BacktestResult {
    let (total_pnl, sharpe, max_dd, win_rate_val, trade_count) = trading_summary
        .instruments
        .values()
        .fold(
            (Decimal::ZERO, Decimal::ZERO, Decimal::ZERO, None, 0u32),
            |(pnl, _sharpe, max_dd, win_rate, trades), tear_sheet| {
                let sheet_pnl = pnl + tear_sheet.pnl;
                let sheet_sharpe = tear_sheet.sharpe_ratio.value;

                let sheet_max_dd = tear_sheet
                    .pnl_drawdown_max
                    .as_ref()
                    .map_or(max_dd, |dd| {
                        let dd_val = dd.0.value.abs();
                        if dd_val > max_dd { dd_val } else { max_dd }
                    });

                let sheet_win_rate = tear_sheet.win_rate.as_ref().map(|wr| wr.value);

                let sheet_trades = tear_sheet
                    .profit_factor
                    .as_ref()
                    .map_or(trades, |_| trades + 1);

                (
                    sheet_pnl,
                    sheet_sharpe,
                    sheet_max_dd,
                    sheet_win_rate.or(win_rate),
                    sheet_trades,
                )
            },
        );

    let sharpe_f64 = sharpe.try_into().unwrap_or(0.0f64);
    let win_rate_f64: f64 = win_rate_val
        .and_then(|v| v.try_into().ok())
        .unwrap_or(0.0);

    BacktestResult::builder()
        .pnl(total_pnl)
        .sharpe_ratio(sharpe_f64)
        .max_drawdown(max_dd)
        .win_rate(win_rate_f64)
        .trade_count(trade_count)
        .timeframe(timeframe)
        .build()
}

/// Build an `Instrument` for a given contract ID using simulated exchange.
///
/// For MVP, all instruments are treated as spot instruments on a simulated exchange.
fn build_instrument(contract_id: &str) -> Instrument<ExchangeId, Asset> {
    // Parse contract_id as "base_quote" (e.g., "btc_usdt") or use as-is
    let (base, quote) = contract_id
        .split_once('_')
        .unwrap_or((contract_id, "usdt"));

    Instrument::spot(
        ExchangeId::Simulated,
        format!("simulated-{base}_{quote}"),
        contract_id,
        Underlying::new(
            Asset::new_from_exchange(base),
            Asset::new_from_exchange(quote),
        ),
        None,
    )
}

impl BarterBacktester {
    /// Shared backtest execution logic.
    ///
    /// Loads the strategy via the executor, builds the barter engine with
    /// `CandleInstrumentData`, and runs the backtest to extract metrics.
    async fn run_with_market_data(
        &self,
        strategy_artifact: &[u8],
        contract_id: &str,
        timeframe: Timeframe,
        market_data: MarketDataInMemory<DataKind>,
    ) -> Result<BacktestResult, BacktestError> {
        // Load WASM strategy via executor
        let handle = self.executor.load(strategy_artifact).map_err(|e| {
            BacktestError::ExecutionFailed {
                message: format!("failed to load strategy: {e}"),
            }
        })?;
        let strategy = BarterStrategy::new(handle, timeframe);

        let instrument = build_instrument(contract_id);
        let instruments = IndexedInstruments::new(vec![instrument]);

        // Build the initial account snapshot with configured capital
        let (_, quote_name) = contract_id
            .split_once('_')
            .unwrap_or((contract_id, "usdt"));

        let initial_state = UnindexedAccountSnapshot {
            exchange: ExchangeId::Simulated,
            balances: vec![barter_execution::balance::AssetBalance::new(
                AssetNameExchange::from(quote_name),
                Balance::new(self.initial_capital, self.initial_capital),
                chrono::Utc::now(),
            )],
            instruments: vec![],
        };

        let execution_config = ExecutionConfig::Mock(MockExecutionConfig::new(
            ExchangeId::Simulated,
            initial_state,
            0, // zero latency for backtest
            self.fees_percent,
        ));

        let engine_state: BacktestEngineState = EngineState::builder(
            &instruments,
            DefaultGlobalData,
            |_| CandleInstrumentData::default(),
        )
        .trading_state(TradingState::Enabled)
        .build();

        let args_constant = Arc::new(BacktestArgsConstant {
            instruments,
            executions: vec![execution_config],
            market_data,
            summary_interval: Daily,
            engine_state,
        });

        let args_dynamic = BacktestArgsDynamic {
            id: SmolStr::new(contract_id),
            risk_free_return: Decimal::new(5, 2), // 0.05 risk-free rate
            strategy,
            risk: BtRisk::default(),
        };

        let multi_summary = run_backtests(args_constant, vec![args_dynamic])
            .await
            .map_err(|e| BacktestError::ExecutionFailed {
                message: format!("barter backtest engine error: {e}"),
            })?;

        let summary = multi_summary
            .summaries
            .into_iter()
            .next()
            .ok_or_else(|| BacktestError::ExecutionFailed {
                message: "no backtest summary produced".to_string(),
            })?;

        Ok(extract_metrics(&summary.trading_summary, timeframe))
    }
}

#[async_trait]
impl Backtester for BarterBacktester {
    async fn run(
        &self,
        strategy_artifact: &[u8],
        contract_id: &str,
        timeframe: Timeframe,
    ) -> Result<BacktestResult, BacktestError> {
        let candle_rows = self
            .store
            .query_candles(
                contract_id,
                "1m",
                self.backtest_start,
                self.backtest_end,
            )
            .await
            .map_err(|e| BacktestError::ExecutionFailed {
                message: format!("failed to query market data: {e}"),
            })?;

        if candle_rows.is_empty() {
            return Err(BacktestError::ExecutionFailed {
                message: format!("no market data found for {contract_id}"),
            });
        }

        let events: Vec<_> = candle_rows
            .iter()
            .map(|row| {
                let candle = Candle {
                    close_time: row.ts,
                    open: row.open,
                    high: row.high,
                    low: row.low,
                    close: row.close,
                    volume: row.volume,
                    trade_count: u64::from(row.trade_count.cast_unsigned()),
                };

                ReconnectEvent::Item(MarketEvent {
                    time_exchange: row.ts,
                    time_received: row.ts,
                    exchange: ExchangeId::Simulated,
                    instrument: InstrumentIndex(0),
                    kind: DataKind::Candle(candle),
                })
            })
            .collect();

        let market_data = MarketDataInMemory::new(Arc::new(events));
        self.run_with_market_data(strategy_artifact, contract_id, timeframe, market_data)
            .await
    }
}
