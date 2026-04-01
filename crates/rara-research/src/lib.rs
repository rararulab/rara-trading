//! Research engine — hypothesis generation, backtesting, and feedback loops.

pub mod backtest_pool;
pub mod backtester;
pub mod barter_backtester;
pub mod barter_strategy;
pub mod candle_instrument_data;
pub mod compiler;
pub mod feedback_gen;
pub mod hypothesis_gen;
pub mod prompt_renderer;
pub mod research_loop;
pub mod strategy_coder;
pub mod strategy_executor;
pub mod strategy_manager;
pub mod strategy_promoter;
pub mod strategy_registry;
pub mod strategy_store;
pub mod trace;
pub mod wasm_executor;
pub mod wasm_strategy_manager;
