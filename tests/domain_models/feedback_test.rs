use rust_decimal_macros::dec;

use rara_trading::domain::feedback::{FeedbackDecision, StrategyMetrics, StrategyReport};

#[test]
fn retire_decision_indicates_retrain() {
    let metrics = StrategyMetrics::builder()
        .pnl(dec!(-500))
        .sharpe_ratio(-0.5)
        .max_drawdown(dec!(0.25))
        .win_rate(0.3)
        .trade_count(100)
        .build();

    let report = StrategyReport::builder()
        .strategy_id("strat-001")
        .strategy_version(1)
        .window_start(jiff::Timestamp::UNIX_EPOCH)
        .window_end(jiff::Timestamp::now())
        .metrics(metrics)
        .sentinel_events(vec![])
        .decision(FeedbackDecision::Retire)
        .reason("Sustained losses")
        .build();

    assert!(report.should_trigger_retrain());
}

#[test]
fn promote_decision_does_not_retrain() {
    let metrics = StrategyMetrics::builder()
        .pnl(dec!(1000))
        .sharpe_ratio(2.0)
        .max_drawdown(dec!(0.05))
        .win_rate(0.65)
        .trade_count(200)
        .build();

    let report = StrategyReport::builder()
        .strategy_id("strat-002")
        .strategy_version(1)
        .window_start(jiff::Timestamp::UNIX_EPOCH)
        .window_end(jiff::Timestamp::now())
        .metrics(metrics)
        .sentinel_events(vec![])
        .decision(FeedbackDecision::Promote)
        .reason("Strong performance")
        .build();

    assert!(!report.should_trigger_retrain());
}
