use uuid::Uuid;

use rara_trading::domain::research::{Experiment, ExperimentStatus, Hypothesis, HypothesisFeedback};

#[test]
fn hypothesis_with_parent_forms_chain() {
    let h1 = Hypothesis::builder()
        .text("BTC trends after halving")
        .reason("Historical pattern")
        .build();

    let h2 = Hypothesis::builder()
        .text("BTC trends within 30 days of halving")
        .reason("Refined time window")
        .parent(h1.id())
        .build();

    assert!(h1.parent().is_none());
    assert_eq!(h2.parent(), Some(h1.id()));
}

#[test]
fn experiment_status_default_pending() {
    let exp = Experiment::builder()
        .hypothesis_id(Uuid::new_v4())
        .strategy_code("fn run() {}")
        .build();

    assert_eq!(exp.status(), ExperimentStatus::Pending);
}

#[test]
fn feedback_decision() {
    let fb = HypothesisFeedback::builder()
        .experiment_id(Uuid::new_v4())
        .decision(false)
        .reason("Poor Sharpe ratio")
        .observations("High variance in returns")
        .build();

    assert!(!fb.decision());
    assert_eq!(fb.reason(), "Poor Sharpe ratio");
}
