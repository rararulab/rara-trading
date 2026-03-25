//! Research types — hypotheses, experiments, and feedback loops.

use bon::Builder;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A trading hypothesis to be tested experimentally.
#[derive(Debug, Clone, Serialize, Deserialize, Builder)]
pub struct Hypothesis {
    #[builder(default = Uuid::new_v4())]
    id: Uuid,
    #[builder(into)]
    text: String,
    #[builder(into)]
    reason: String,
    /// What was observed in prior experiment results.
    #[builder(default, into)]
    observation: String,
    /// Domain knowledge applied when forming this hypothesis.
    #[builder(default, into)]
    knowledge: String,
    parent: Option<Uuid>,
    #[builder(default = jiff::Timestamp::now())]
    created_at: jiff::Timestamp,
}

impl Hypothesis {
    /// Returns the hypothesis identifier.
    pub const fn id(&self) -> Uuid {
        self.id
    }

    /// Returns the hypothesis text.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Returns the reasoning behind this hypothesis.
    pub fn reason(&self) -> &str {
        &self.reason
    }

    /// Returns the observation from prior results.
    pub fn observation(&self) -> &str {
        &self.observation
    }

    /// Returns the domain knowledge applied.
    pub fn knowledge(&self) -> &str {
        &self.knowledge
    }

    /// Returns the parent hypothesis ID if this is a refinement.
    pub const fn parent(&self) -> Option<Uuid> {
        self.parent
    }
}

/// Lifecycle status of an experiment.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
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
    id: Uuid,
    hypothesis_id: Uuid,
    #[builder(into)]
    strategy_code: String,
    #[builder(default)]
    status: ExperimentStatus,
    backtest_result: Option<BacktestResult>,
    #[builder(default = jiff::Timestamp::now())]
    created_at: jiff::Timestamp,
}

impl Experiment {
    /// Returns the experiment identifier.
    pub const fn id(&self) -> Uuid {
        self.id
    }

    /// Returns the hypothesis this experiment tests.
    pub const fn hypothesis_id(&self) -> Uuid {
        self.hypothesis_id
    }

    /// Returns the current experiment status.
    pub const fn status(&self) -> ExperimentStatus {
        self.status
    }

    /// Returns the strategy source code.
    pub fn strategy_code(&self) -> &str {
        &self.strategy_code
    }

    /// Returns the backtest result, if available.
    pub const fn backtest_result(&self) -> Option<&BacktestResult> {
        self.backtest_result.as_ref()
    }
}

/// Results from a backtest run.
#[derive(Debug, Clone, Serialize, Deserialize, Builder)]
pub struct BacktestResult {
    /// Profit and loss.
    pnl: Decimal,
    /// Sharpe ratio.
    sharpe_ratio: f64,
    /// Maximum drawdown.
    max_drawdown: Decimal,
    /// Win rate as a fraction.
    win_rate: f64,
    /// Total number of trades.
    trade_count: u32,
}

impl BacktestResult {
    /// Returns the profit and loss.
    pub const fn pnl(&self) -> Decimal {
        self.pnl
    }

    /// Returns the Sharpe ratio.
    pub const fn sharpe_ratio(&self) -> f64 {
        self.sharpe_ratio
    }

    /// Returns the maximum drawdown.
    pub const fn max_drawdown(&self) -> Decimal {
        self.max_drawdown
    }

    /// Returns the win rate as a fraction.
    pub const fn win_rate(&self) -> f64 {
        self.win_rate
    }

    /// Returns the total number of trades.
    pub const fn trade_count(&self) -> u32 {
        self.trade_count
    }
}

/// Feedback on an experiment guiding the next research iteration.
#[derive(Debug, Clone, Serialize, Deserialize, Builder)]
pub struct HypothesisFeedback {
    experiment_id: Uuid,
    decision: bool,
    #[builder(into)]
    reason: String,
    #[builder(into)]
    observations: String,
    /// Whether the hypothesis was validated, refuted, or inconclusive.
    #[builder(default, into)]
    hypothesis_evaluation: String,
    /// LLM suggestion for the next research round.
    new_hypothesis: Option<String>,
    /// Summary of code changes from the previous experiment.
    #[builder(default, into)]
    code_change_summary: String,
    #[builder(default = jiff::Timestamp::now())]
    created_at: jiff::Timestamp,
}

impl HypothesisFeedback {
    /// Returns the experiment this feedback relates to.
    pub const fn experiment_id(&self) -> Uuid {
        self.experiment_id
    }

    /// Returns whether the hypothesis was accepted.
    pub const fn decision(&self) -> bool {
        self.decision
    }

    /// Returns the reasoning behind the decision.
    pub fn reason(&self) -> &str {
        &self.reason
    }

    /// Returns the observations recorded during evaluation.
    pub fn observations(&self) -> &str {
        &self.observations
    }

    /// Returns the hypothesis evaluation result.
    pub fn hypothesis_evaluation(&self) -> &str {
        &self.hypothesis_evaluation
    }

    /// Returns an optional suggestion for the next hypothesis.
    pub fn new_hypothesis(&self) -> Option<&str> {
        self.new_hypothesis.as_deref()
    }

    /// Returns a summary of code changes from the previous experiment.
    pub fn code_change_summary(&self) -> &str {
        &self.code_change_summary
    }
}
