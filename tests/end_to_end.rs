//! End-to-end integration test proving the closed loop:
//! Research → Trading → Feedback.

use std::sync::Arc;

use rust_decimal::Decimal;
use serde_json::json;

use rara_trading::domain::event::Event;
use rara_trading::domain::feedback::FeedbackDecision;
use rara_trading::domain::research::BacktestResult;
use rara_trading::domain::trading::{ActionType, OrderType, Side, StagedAction, TradingCommit};
use rara_trading::event_bus::bus::EventBus;
use rara_trading::feedback::aggregator::MetricsAggregator;
use rara_trading::feedback::engine::FeedbackBridge;
use rara_trading::feedback::evaluator::StrategyEvaluator;
use rara_trading::agent::backend::{CliBackend, OutputFormat, PromptMode};
use rara_trading::agent::executor::CliExecutor;
use rara_trading::research::backtester::MockBacktester;
use rara_trading::research::research_loop::ResearchLoop;
use rara_trading::research::trace::Trace;
use rara_trading::trading::broker::OrderStatus;
use rara_trading::trading::brokers::paper::PaperBroker;
use rara_trading::trading::engine::TradingEngine;
use rara_trading::trading::guard_pipeline::GuardPipeline;

fn printf_executor(response: &str) -> CliExecutor {
    CliExecutor::new(CliBackend {
        command: "sh".to_string(),
        args: vec!["-c".to_string(), format!("printf '{response}\\n'")],
        prompt_mode: PromptMode::Arg,
        prompt_flag: None,
        output_format: OutputFormat::Text,
        env_vars: vec![],
    })
}

#[tokio::test]
async fn full_research_to_feedback_loop() {
    // --- Setup ---
    let bus_dir = tempfile::tempdir().unwrap();
    let trace_dir = tempfile::tempdir().unwrap();
    let event_bus = Arc::new(EventBus::open(bus_dir.path()).unwrap());
    let trace = Trace::open(trace_dir.path()).unwrap();

    // --- Phase 1: Research — generate a candidate strategy ---
    let executor = printf_executor("momentum crossover\nSMA 20/50 crossover signals trend change");

    let good_backtest = BacktestResult::builder()
        .pnl(Decimal::new(5000, 0))
        .sharpe_ratio(2.5)
        .max_drawdown(Decimal::new(5, 2))
        .win_rate(0.65)
        .trade_count(100)
        .build();

    let mock_bt = MockBacktester::new(vec![good_backtest]);
    let research = ResearchLoop::new(executor, mock_bt, trace, Arc::clone(&event_bus));

    let iteration = research.run_iteration("BTC trending up").await.unwrap();
    assert!(iteration.accepted, "research should accept good backtest");

    // Verify research events were published
    let research_events = event_bus.store().read_topic("research", 0, 100).unwrap();
    assert!(
        research_events.len() >= 2,
        "expected at least hypothesis.created + experiment.completed"
    );

    // --- Phase 2: Trading — execute via TradingEngine with PaperBroker ---
    let strategy_id = "momentum-crossover-v1";
    let strategy_version = 1u32;

    let broker = PaperBroker::new(Decimal::new(50_000, 0));
    let pipeline = GuardPipeline::new(vec![]);
    let engine = TradingEngine::new(
        Box::new(broker),
        pipeline,
        Arc::clone(&event_bus),
    );

    // Simulate executing trades produced by the strategy
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

    // Publish additional simulated fill events with realized PnL
    // (the TradingEngine's fills don't include realized_pnl, so we add some)
    // Use small positive values with slight variance to ensure:
    //   - win_rate > 0.5, sharpe > 1.5, drawdown stays small
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

    // Verify trading events exist
    let trading_events = event_bus.store().read_topic("trading", 0, 100).unwrap();
    assert!(
        trading_events.len() >= 2,
        "expected submitted + filled events from engine plus simulated fills"
    );

    // --- Phase 3: Feedback — evaluate the strategy ---
    let aggregator = MetricsAggregator::new(Arc::clone(&event_bus));
    let evaluator = StrategyEvaluator::new(1.5, Decimal::new(20, 2), 5);
    let feedback_bridge = FeedbackBridge::new(aggregator, evaluator, Arc::clone(&event_bus));

    let window_start = jiff::Timestamp::from_millisecond(0).unwrap();
    let window_end = jiff::Timestamp::now();

    let report = feedback_bridge
        .evaluate_strategy(
            strategy_id,
            strategy_version,
            window_start,
            window_end,
            vec![],
        )
        .unwrap();

    // With 9 positive PnL trades + 1 engine fill (0 pnl) = 10 trades,
    // all positive → win_rate=0.9, sharpe well above threshold, low drawdown
    assert_eq!(
        report.decision(),
        FeedbackDecision::Promote,
        "strategy with strong results should be promoted"
    );

    // --- Phase 4: Verify the full event chain ---
    let feedback_events = event_bus.store().read_topic("feedback", 0, 100).unwrap();
    assert_eq!(feedback_events.len(), 1);
    assert_eq!(
        feedback_events[0].event_type(),
        "feedback.strategy.promote"
    );
    assert_eq!(
        feedback_events[0].strategy_id(),
        Some(strategy_id),
        "feedback event should reference the strategy"
    );

    // Verify all three topic families have events (the closed loop)
    assert!(!research_events.is_empty(), "research events exist");
    assert!(!trading_events.is_empty(), "trading events exist");
    assert!(!feedback_events.is_empty(), "feedback events exist");
}
