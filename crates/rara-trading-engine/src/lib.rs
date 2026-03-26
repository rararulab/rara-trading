//! Trading engine module — broker abstraction, guards, and orchestration.

pub mod account_manager;
pub mod binding;
pub mod broker;
pub mod brokers;
pub mod engine;
pub mod guard_pipeline;
pub mod guards;
pub mod health;
pub mod signal_loop;
pub mod trading_git;
pub mod uta;
