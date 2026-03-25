//! Feedback loop — aggregates trading metrics, evaluates strategy performance,
//! and publishes lifecycle decisions.

pub mod aggregator;
pub mod engine;
pub mod evaluator;
pub mod retrain;
