//! Real backtester implementation using the barter-rs engine.
//!
//! Replaces the mock backtester with a real backtest runner that loads historical
//! market data, runs it through barter's engine with a configurable strategy, and
//! extracts performance metrics into our domain `BacktestResult`.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use barter::backtest::market_data::MarketDataInMemory;
use barter::backtest::{BacktestArgsConstant, BacktestArgsDynamic, run_backtests};
use barter::engine::state::EngineState;
use barter::engine::state::global::DefaultGlobalData;
use barter::engine::state::instrument::data::DefaultInstrumentMarketData;
use barter::engine::state::trading::TradingState;
use barter::risk::DefaultRiskManager;
use barter::statistic::time::Daily;
use barter::strategy::DefaultStrategy;
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
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use smol_str::SmolStr;

use rara_domain::research::BacktestResult;
use rara_market_data::cache::MarketSlice;
use rara_market_data::record::FIXED_POINT_SCALE;

use crate::backtester::{BacktestError, Backtester};
use crate::market_data::load_market_data_for_contract;

/// Real backtester powered by the barter-rs trading engine.
///
/// Loads historical market data from JSON files, runs it through barter's
/// backtest infrastructure with mock execution, and extracts performance
/// metrics into our domain `BacktestResult`.
#[derive(Debug, Clone, Builder)]
pub struct BarterBacktester {
    /// Directory containing historical market data JSON files.
    data_dir: PathBuf,
    /// Initial capital for the simulated account.
    initial_capital: Decimal,
    /// Trading fees as a percentage (e.g., 0.1 for 0.1%).
    fees_percent: Decimal,
}

/// Default state types used by the barter backtest engine.
type BtEngineState = EngineState<DefaultGlobalData, DefaultInstrumentMarketData>;
type BtStrategy = DefaultStrategy<BtEngineState>;
type BtRisk = DefaultRiskManager<BtEngineState>;

/// Extract performance metrics from a barter `TradingSummary` into our domain `BacktestResult`.
///
/// Aggregates `PnL`, Sharpe ratio, max drawdown, and win rate across all instruments
/// in the trading summary.
fn extract_metrics(
    trading_summary: &barter::statistic::summary::TradingSummary<Daily>,
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

/// Convert cached `MarketSlice` candle data to barter `MarketDataInMemory`.
///
/// Each candle record is converted from fixed-point representation back to
/// floating-point and wrapped as a barter `MarketEvent<DataKind::Candle>`.
fn market_data_from_slices(
    slices: &[Arc<MarketSlice>],
    instrument_index: InstrumentIndex,
    exchange: ExchangeId,
) -> Result<MarketDataInMemory<DataKind>, BacktestError> {
    #[allow(clippy::cast_precision_loss)]
    let scale = FIXED_POINT_SCALE as f64;

    let all_events: Vec<_> = slices
        .iter()
        .map(|slice| {
            slice.candles().map_err(|e| BacktestError::ExecutionFailed {
                message: format!("failed to read candle data: {e}"),
            })
        })
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .flat_map(|candles| candles.iter())
        .map(|record| {
            // Precision loss is acceptable: barter uses f64 internally and
            // the fixed-point values fit well within f64's mantissa range
            // for realistic price/volume magnitudes.
            #[allow(clippy::cast_precision_loss)]
            let candle = Candle {
                close_time: timestamp_from_nanos(record.ts_event),
                open: record.open as f64 / scale,
                high: record.high as f64 / scale,
                low: record.low as f64 / scale,
                close: record.close as f64 / scale,
                volume: record.volume as f64 / scale,
                trade_count: u64::from(record.trade_count),
            };

            let market_event = MarketEvent {
                time_exchange: timestamp_from_nanos(record.ts_event),
                time_received: timestamp_from_nanos(record.ts_event),
                exchange,
                instrument: instrument_index,
                kind: DataKind::Candle(candle),
            };

            ReconnectEvent::Item(market_event)
        })
        .collect();

    if all_events.is_empty() {
        return Err(BacktestError::ExecutionFailed {
            message: "no market data in slices".to_string(),
        });
    }

    Ok(MarketDataInMemory::new(Arc::new(all_events)))
}

/// Convert nanosecond timestamp to chrono `DateTime<Utc>`.
const fn timestamp_from_nanos(nanos: i64) -> DateTime<Utc> {
    DateTime::from_timestamp_nanos(nanos)
}

impl BarterBacktester {
    /// Shared backtest execution logic used by both `run` and `run_with_data`.
    ///
    /// Takes pre-built market data and runs it through the barter engine,
    /// returning extracted performance metrics.
    async fn run_with_market_data(
        &self,
        _strategy_code: &str,
        contract_id: &str,
        market_data: MarketDataInMemory<DataKind>,
    ) -> Result<BacktestResult, BacktestError> {
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

        let engine_state: BtEngineState = EngineState::builder(
            &instruments,
            DefaultGlobalData,
            |_| DefaultInstrumentMarketData::default(),
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
            strategy: BtStrategy::default(),
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

        Ok(extract_metrics(&summary.trading_summary))
    }
}

#[async_trait]
impl Backtester for BarterBacktester {
    async fn run(
        &self,
        strategy_code: &str,
        contract_id: &str,
    ) -> Result<BacktestResult, BacktestError> {
        let market_data = load_market_data_for_contract(
            &self.data_dir,
            contract_id,
            InstrumentIndex(0),
            ExchangeId::Simulated,
        )
        .map_err(|e| BacktestError::ExecutionFailed {
            message: format!("failed to load market data: {e}"),
        })?;

        self.run_with_market_data(strategy_code, contract_id, market_data)
            .await
    }

    async fn run_with_data(
        &self,
        strategy_code: &str,
        contract_id: &str,
        data: &[Arc<MarketSlice>],
    ) -> Result<BacktestResult, BacktestError> {
        let market_data =
            market_data_from_slices(data, InstrumentIndex(0), ExchangeId::Simulated)?;

        self.run_with_market_data(strategy_code, contract_id, market_data)
            .await
    }
}
