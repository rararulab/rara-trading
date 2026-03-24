//! End-to-end integration test proving the closed loop:
//! Research → Trading → Feedback.

use std::sync::Arc;

use rust_decimal::Decimal;
use serde_json::json;

use rara_trading::domain::event::Event;
use rara_trading::domain::feedback::FeedbackDecision;
use rara_trading::domain::research::{Experiment, Hypothesis, HypothesisFeedback};
use rara_trading::domain::trading::{ActionType, OrderType, Side, StagedAction, TradingCommit};
use rara_trading::event_bus::bus::EventBus;
use rara_trading::feedback::aggregator::MetricsAggregator;
use rara_trading::feedback::engine::FeedbackBridge;
use rara_trading::feedback::evaluator::StrategyEvaluator;
use rara_trading::research::trace::Trace;
use rara_trading::trading::broker::OrderStatus;
use rara_trading::trading::brokers::paper::PaperBroker;
use rara_trading::trading::engine::TradingEngine;
use rara_trading::trading::guard_pipeline::GuardPipeline;

/// Build research artifacts (hypothesis, experiment, feedback) and publish
/// the corresponding domain events. Returns the count of research events.
fn run_research_phase(event_bus: &EventBus, trace: &Trace) -> usize {
    let hypothesis = Hypothesis::builder()
        .text("momentum crossover")
        .reason("SMA 20/50 crossover signals trend change")
        .build();

    trace.save_hypothesis(&hypothesis).unwrap();

    let hyp_event = Event::builder()
        .event_type("research.hypothesis.created")
        .source("research_loop")
        .correlation_id(uuid::Uuid::new_v4().to_string())
        .payload(json!({ "hypothesis_id": hypothesis.id().to_string() }))
        .build();
    event_bus.publish(&hyp_event).unwrap();

    let experiment = Experiment::builder()
        .hypothesis_id(hypothesis.id())
        .strategy_code("fn strategy() { /* SMA crossover */ }")
        .build();

    trace.save_experiment(&experiment).unwrap();

    let feedback = HypothesisFeedback::builder()
        .experiment_id(experiment.id())
        .decision(true)
        .reason("Accepted: sharpe=2.50, max_drawdown=0.05")
        .observations("pnl=5000, win_rate=0.65, trades=100")
        .build();

    trace.save_feedback(&feedback).unwrap();

    let exp_event = Event::builder()
        .event_type("research.experiment.completed")
        .source("research_loop")
        .correlation_id(uuid::Uuid::new_v4().to_string())
        .payload(json!({
            "experiment_id": experiment.id().to_string(),
            "accepted": true,
        }))
        .build();
    event_bus.publish(&exp_event).unwrap();

    let candidate_event = Event::builder()
        .event_type("research.strategy.candidate")
        .source("research_loop")
        .correlation_id(uuid::Uuid::new_v4().to_string())
        .payload(json!({
            "experiment_id": experiment.id().to_string(),
            "hypothesis_id": hypothesis.id().to_string(),
        }))
        .build();
    event_bus.publish(&candidate_event).unwrap();

    event_bus
        .store()
        .read_topic("research", 0, 100)
        .unwrap()
        .len()
}

/// Execute a trading commit via paper broker, then publish simulated fill
/// events with positive `PnL`. Returns the count of trading events.
async fn run_trading_phase(
    event_bus: &Arc<EventBus>,
    strategy_id: &str,
    strategy_version: u32,
) -> usize {
    let broker = PaperBroker::new(Decimal::new(50_000, 0));
    let pipeline = GuardPipeline::new(vec![]);
    let engine = TradingEngine::new(
        Box::new(broker),
        pipeline,
        Arc::clone(event_bus),
    );

    let commit = TradingCommit::builder()
        .message("golden cross detected on BTC")
        .strategy_id(strategy_id)
        .strategy_version(strategy_version)
        .actions(vec![StagedAction::builder()
            .action_type(ActionType::PlaceOrder)
            .contract_id("BTC-USD")
            .side(Side::Buy)
            .quantity(Decimal::ONE)
            .order_type(OrderType::Market)
            .build()])
        .build();

    let results = engine.execute_commit(commit).await.unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].status, OrderStatus::Filled);

    // Publish simulated fill events with positive PnL to ensure
    // win_rate > 0.5, sharpe > 1.5, and small drawdown
    let sim_pnls = ["120", "80", "110", "90", "130", "95", "105", "115", "100"];
    for (i, pnl) in sim_pnls.iter().enumerate() {
        let event = Event::builder()
            .event_type("trading.order.filled")
            .source("test-harness")
            .correlation_id(format!("sim-{i}"))
            .strategy_id(strategy_id.to_owned())
            .strategy_version(strategy_version)
            .payload(json!({ "realized_pnl": pnl }))
            .build();
        event_bus.publish(&event).unwrap();
    }

    event_bus
        .store()
        .read_topic("trading", 0, 100)
        .unwrap()
        .len()
}

/// Evaluate the strategy through the feedback bridge and assert promotion.
/// Returns the count of feedback events.
fn run_feedback_phase(
    event_bus: &Arc<EventBus>,
    strategy_id: &str,
    strategy_version: u32,
) -> usize {
    let aggregator = MetricsAggregator::new(Arc::clone(event_bus));
    let evaluator = StrategyEvaluator::new(1.5, Decimal::new(20, 2), 5);
    let feedback_bridge = FeedbackBridge::new(aggregator, evaluator, Arc::clone(event_bus));

    let window_start = jiff::Timestamp::from_millisecond(0).unwrap();
    let window_end = jiff::Timestamp::now();

    let report = feedback_bridge
        .evaluate_strategy(strategy_id, strategy_version, window_start, window_end, vec![])
        .unwrap();

    // With 9 positive PnL trades + 1 engine fill (0 pnl) = 10 trades,
    // all positive → win_rate=0.9, sharpe well above threshold, low drawdown
    assert_eq!(
        report.decision(),
        FeedbackDecision::Promote,
        "strategy with strong results should be promoted"
    );

    let events = event_bus
        .store()
        .read_topic("feedback", 0, 100)
        .unwrap();

    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event_type(), "feedback.strategy.promote");
    assert_eq!(
        events[0].strategy_id(),
        Some(strategy_id),
        "feedback event should reference the strategy"
    );

    events.len()
}

#[tokio::test]
async fn full_research_to_feedback_loop() {
    let bus_dir = tempfile::tempdir().unwrap();
    let trace_dir = tempfile::tempdir().unwrap();
    let event_bus = Arc::new(EventBus::open(bus_dir.path()).unwrap());
    let trace = Trace::open(trace_dir.path()).unwrap();

    let strategy_id = "momentum-crossover-v1";
    let strategy_version = 1u32;

    let research_count = run_research_phase(&event_bus, &trace);
    assert!(research_count >= 2, "expected at least hypothesis.created + experiment.completed");

    let trading_count = run_trading_phase(&event_bus, strategy_id, strategy_version).await;
    assert!(trading_count >= 2, "expected submitted + filled events from engine plus simulated fills");

    let feedback_count = run_feedback_phase(&event_bus, strategy_id, strategy_version);

    // Verify all three topic families have events (the closed loop)
    assert!(research_count > 0, "research events exist");
    assert!(trading_count > 0, "trading events exist");
    assert!(feedback_count > 0, "feedback events exist");
}
