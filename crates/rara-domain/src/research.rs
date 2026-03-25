//! Research types — hypotheses, experiments, and feedback loops.

use bon::Builder;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use strum::{Display, EnumString};
use uuid::Uuid;

/// A trading hypothesis to be tested experimentally.
#[derive(Debug, Clone, Serialize, Deserialize, Builder)]
pub struct Hypothesis {
    #[builder(default = Uuid::new_v4())]
    pub id: Uuid,
    #[builder(into)]
    pub text: String,
    #[builder(into)]
    pub reason: String,
    /// What was observed in prior experiment results.
    #[builder(default, into)]
    pub observation: String,
    /// Domain knowledge applied when forming this hypothesis.
    #[builder(default, into)]
    pub knowledge: String,
    pub parent: Option<Uuid>,
    #[builder(default = jiff::Timestamp::now())]
    pub created_at: jiff::Timestamp,
}

/// Lifecycle status of an experiment.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, Display, EnumString)]
pub enum ExperimentStatus {
    /// Awaiting execution.
    #[default]
    Pending,
    /// Strategy code is being generated.
    Coding,
    /// Backtest is running.
    Backtesting,
    /// Results are being evaluated.
    Evaluating,
    /// Experiment finished successfully.
    Completed,
    /// Experiment failed.
    Failed,
}

/// An experiment that tests a hypothesis via backtesting.
#[derive(Debug, Clone, Serialize, Deserialize, Builder)]
pub struct Experiment {
    #[builder(default = Uuid::new_v4())]
    pub id: Uuid,
    pub hypothesis_id: Uuid,
    #[builder(into)]
    pub strategy_code: String,
    #[builder(default)]
    pub status: ExperimentStatus,
    pub backtest_result: Option<BacktestResult>,
    #[builder(default = jiff::Timestamp::now())]
    pub created_at: jiff::Timestamp,
}

/// Results from a backtest run.
#[derive(Debug, Clone, Serialize, Deserialize, Builder)]
pub struct BacktestResult {
    /// Profit and loss.
    pub pnl: Decimal,
    /// Sharpe ratio.
    pub sharpe_ratio: f64,
    /// Maximum drawdown.
    pub max_drawdown: Decimal,
    /// Win rate as a fraction.
    pub win_rate: f64,
    /// Total number of trades.
    pub trade_count: u32,
}

/// Feedback on an experiment guiding the next research iteration.
#[derive(Debug, Clone, Serialize, Deserialize, Builder)]
pub struct HypothesisFeedback {
    pub experiment_id: Uuid,
    pub decision: bool,
    #[builder(into)]
    pub reason: String,
    #[builder(into)]
    pub observations: String,
    /// Whether the hypothesis was validated, refuted, or inconclusive.
    #[builder(default, into)]
    pub hypothesis_evaluation: String,
    /// LLM suggestion for the next research round.
    pub new_hypothesis: Option<String>,
    /// Summary of code changes from the previous experiment.
    #[builder(default, into)]
    pub code_change_summary: String,
    #[builder(default = jiff::Timestamp::now())]
    pub created_at: jiff::Timestamp,
}
