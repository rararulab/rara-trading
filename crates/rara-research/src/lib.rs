//! Research engine — hypothesis generation, backtesting, and feedback loops.

pub mod backtester;
pub mod compiler;
pub mod barter_backtester;
pub mod feedback_gen;
pub mod hypothesis_gen;
pub mod market_data;
pub mod prompt_renderer;
pub mod research_loop;
pub mod strategy_coder;
pub mod runtime;
pub mod trace;
