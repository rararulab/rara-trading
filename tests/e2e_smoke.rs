//! End-to-end smoke tests validating the research -> paper trading -> feedback
//! loop works across crate boundaries.
//!
//! These tests exercise real implementations (sled stores, paper broker, guard
//! pipeline) with temporary directories — no mocks.

use rara_domain::{
    event::{Event, EventType},
    research::{
        BacktestResult, Experiment, Hypothesis, HypothesisFeedback, ResearchStrategy,
        ResearchStrategyStatus,
    },
    trading::{ActionType, OrderType, Side, StagedAction, TradingCommit},
};
use rara_event_bus::bus::EventBus;
use rara_research::{
    strategy_store::StrategyStore,
    trace::{DagSelection, Trace},
};
use rara_trading_engine::{
    broker::{Broker, OrderStatus},
    brokers::paper::PaperBroker,
    guard_pipeline::GuardPipeline,
    guards::{GuardResult, symbol_whitelist::SymbolWhitelist},
};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde_json::json;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Helpers — reusable experiment/hypothesis builders for the research DAG
// ---------------------------------------------------------------------------

/// Seed a trace with a two-level hypothesis lineage and three experiments:
/// v1 (accepted, Sharpe 0.8), v2 (accepted, Sharpe 1.9), rejected (Sharpe 4.5).
#[allow(clippy::type_complexity)]
fn seed_research_trace(
    dir: &std::path::Path,
) -> (
    Trace,
    Hypothesis,
    Hypothesis,
    Experiment,
    Experiment,
    Experiment,
    u64,
    u64,
) {
    let trace = Trace::open(&dir.join("trace")).unwrap();

    let root_hyp = Hypothesis::builder()
        .text("Mean reversion on BTC 1h candles")
        .reason("Historical mean reversion tendency in crypto")
        .build();
    trace.save_hypothesis(&root_hyp).unwrap();

    let refined_hyp = Hypothesis::builder()
        .text("Mean reversion with volatility filter")
        .reason("Raw mean reversion had high drawdown in volatile regimes")
        .parent(root_hyp.id)
        .build();
    trace.save_hypothesis(&refined_hyp).unwrap();

    let exp_v1 = Experiment::builder()
        .hypothesis_id(root_hyp.id)
        .strategy_code("fn strategy_v1() { /* mean reversion */ }")
        .backtest_result(
            BacktestResult::builder()
                .pnl(dec!(500.0))
                .sharpe_ratio(0.8)
                .max_drawdown(dec!(150.0))
                .win_rate(0.55)
                .trade_count(120)
                .build(),
        )
        .build();
    let fb_v1 = HypothesisFeedback::builder()
        .experiment_id(exp_v1.id)
        .decision(true)
        .reason("Acceptable Sharpe but high drawdown")
        .observations("Drawdown spikes during high-vol periods")
        .build();
    let idx0 = trace
        .record(&exp_v1, &fb_v1, &DagSelection::NewRoot)
        .unwrap();

    let exp_v2 = Experiment::builder()
        .hypothesis_id(refined_hyp.id)
        .strategy_code("fn strategy_v2() { /* mean reversion + vol filter */ }")
        .backtest_result(
            BacktestResult::builder()
                .pnl(dec!(800.0))
                .sharpe_ratio(1.9)
                .max_drawdown(dec!(80.0))
                .win_rate(0.62)
                .trade_count(95)
                .build(),
        )
        .build();
    let fb_v2 = HypothesisFeedback::builder()
        .experiment_id(exp_v2.id)
        .decision(true)
        .reason("Good Sharpe with controlled drawdown")
        .observations("Vol filter reduced drawdown by 47%")
        .build();
    let idx1 = trace
        .record(&exp_v2, &fb_v2, &DagSelection::Latest)
        .unwrap();

    let exp_rejected = Experiment::builder()
        .hypothesis_id(root_hyp.id)
        .strategy_code("fn strategy_overfit() { /* curve-fitted */ }")
        .backtest_result(
            BacktestResult::builder()
                .pnl(dec!(2000.0))
                .sharpe_ratio(4.5)
                .max_drawdown(dec!(20.0))
                .win_rate(0.85)
                .trade_count(30)
                .build(),
        )
        .build();
    let fb_rejected = HypothesisFeedback::builder()
        .experiment_id(exp_rejected.id)
        .decision(false)
        .reason("Overfitting — too few trades, unrealistic metrics")
        .observations("Only 30 trades, likely curve-fitted")
        .build();
    trace
        .record(&exp_rejected, &fb_rejected, &DagSelection::Specific(idx0))
        .unwrap();

    (
        trace,
        root_hyp,
        refined_hyp,
        exp_v1,
        exp_v2,
        exp_rejected,
        idx0,
        idx1,
    )
}

// ---------------------------------------------------------------------------
// 1a. Research trace DAG: hypothesis lineage, SOTA selection, DAG walks
// ---------------------------------------------------------------------------

/// Validates hypothesis lineage, DAG ancestor/children walks, and SOTA
/// selection picking the best accepted experiment by Sharpe ratio.
#[test]
fn research_trace_dag_and_sota_selection() {
    let dir = tempfile::tempdir().unwrap();
    let (trace, root_hyp, refined_hyp, exp_v1, exp_v2, exp_rejected, idx0, idx1) =
        seed_research_trace(dir.path());

    // Hypothesis ancestor chain: refined -> root
    let chain = trace.ancestor_chain(refined_hyp.id).unwrap();
    assert_eq!(
        chain.len(),
        2,
        "refined hypothesis should have root as ancestor"
    );
    assert_eq!(chain[0].id, refined_hyp.id);
    assert_eq!(chain[1].id, root_hyp.id);

    // SOTA should pick exp_v2 (highest Sharpe among accepted, ignoring rejected
    // 4.5)
    let sota = trace.get_sota().unwrap().expect("should have a SOTA");
    assert_eq!(
        sota.0.id, exp_v2.id,
        "SOTA must be the v2 experiment with Sharpe 1.9"
    );
    assert!(sota.1.decision, "SOTA feedback must be accepted");

    // DAG ancestor walk from idx1 -> idx0
    let ancestors = trace.ancestors(idx1).unwrap();
    assert_eq!(ancestors.len(), 2);
    assert_eq!(ancestors[0].0.id, exp_v2.id);
    assert_eq!(ancestors[1].0.id, exp_v1.id);

    // DAG children of root should include v2 (via Latest) and rejected
    let children = trace.children(idx0).unwrap();
    assert_eq!(children.len(), 2, "root node should have 2 children");

    // list_recent returns newest first
    let recent = trace.list_recent(10).unwrap();
    assert_eq!(recent.len(), 3);
    assert_eq!(
        recent[0].1.id, exp_rejected.id,
        "most recent experiment should be listed first"
    );

    // Prompt formatting includes both accepted and rejected
    let prompt = trace.format_for_prompt(10).unwrap();
    assert!(prompt.contains("accepted"));
    assert!(prompt.contains("rejected"));
}

// ---------------------------------------------------------------------------
// 1b. Strategy store: SOTA promotion lifecycle
// ---------------------------------------------------------------------------

/// Validates that a SOTA experiment flows through the strategy store lifecycle:
/// Compiled -> Accepted -> Promoted, with correct status filtering.
#[test]
fn strategy_store_sota_promotion_lifecycle() {
    let dir = tempfile::tempdir().unwrap();
    let (trace, _, refined_hyp, _, exp_v2, ..) = seed_research_trace(dir.path());

    let db = sled::open(dir.path().join("strategy_db")).unwrap();
    let store = StrategyStore::open(&db, &dir.path().join("artifacts")).unwrap();

    // Get SOTA and promote it
    let sota = trace.get_sota().unwrap().expect("should have SOTA");
    assert_eq!(sota.0.id, exp_v2.id);

    let promoted = ResearchStrategy::builder()
        .hypothesis_id(refined_hyp.id)
        .source_code(&exp_v2.strategy_code)
        .build();
    store.save(&promoted).unwrap();

    // Verify lifecycle transitions
    assert_eq!(
        store.get(promoted.id).unwrap().unwrap().status,
        ResearchStrategyStatus::Compiled
    );

    store
        .update_status(promoted.id, ResearchStrategyStatus::Accepted)
        .unwrap();
    store
        .update_status(promoted.id, ResearchStrategyStatus::Promoted)
        .unwrap();

    assert_eq!(
        store.get(promoted.id).unwrap().unwrap().status,
        ResearchStrategyStatus::Promoted
    );

    let promoted_list = store.list(Some(ResearchStrategyStatus::Promoted)).unwrap();
    assert_eq!(promoted_list.len(), 1);
    assert_eq!(promoted_list[0].id, promoted.id);
}

// ---------------------------------------------------------------------------
// 2. Event bus cross-topic routing and consumer offset tracking
// ---------------------------------------------------------------------------

/// Validates that the event bus correctly routes events by topic, maintains
/// ordering within topics, and tracks independent consumer offsets.
#[tokio::test]
async fn event_bus_cross_topic_routing_and_consumer_offsets() {
    let dir = tempfile::tempdir().unwrap();
    let bus = EventBus::open(dir.path()).unwrap();
    let mut rx = bus.subscribe();

    let seq_trading = bus
        .publish(
            &Event::builder()
                .event_type(EventType::TradingOrderSubmitted)
                .source("trading-engine")
                .correlation_id("trade-001")
                .payload(json!({"symbol": "BTC-USD", "side": "buy"}))
                .build(),
        )
        .unwrap();

    let seq_research = bus
        .publish(
            &Event::builder()
                .event_type(EventType::ResearchExperimentCompleted)
                .source("research-loop")
                .correlation_id("exp-001")
                .payload(json!({"sharpe": 1.5}))
                .build(),
        )
        .unwrap();

    let seq_feedback = bus
        .publish(
            &Event::builder()
                .event_type(EventType::FeedbackStrategyPromote)
                .source("feedback-engine")
                .correlation_id("fb-001")
                .strategy_id("strat-alpha".into())
                .strategy_version(3)
                .payload(json!({"action": "promote"}))
                .build(),
        )
        .unwrap();

    let seq_sentinel = bus
        .publish(
            &Event::builder()
                .event_type(EventType::SentinelSignalDetected)
                .source("sentinel")
                .correlation_id("sig-001")
                .payload(json!({"signal": "volatility_spike"}))
                .build(),
        )
        .unwrap();

    let seq_fill = bus
        .publish(
            &Event::builder()
                .event_type(EventType::TradingOrderFilled)
                .source("trading-engine")
                .correlation_id("trade-002")
                .payload(json!({"symbol": "ETH-USD", "price": 3200}))
                .build(),
        )
        .unwrap();

    // Sequence numbers must be monotonically increasing
    assert!(seq_research > seq_trading);
    assert!(seq_feedback > seq_research);
    assert!(seq_sentinel > seq_feedback);
    assert!(seq_fill > seq_sentinel);

    // Broadcast subscriber receives all 5 sequence numbers in order
    for expected in [
        seq_trading,
        seq_research,
        seq_feedback,
        seq_sentinel,
        seq_fill,
    ] {
        assert_eq!(rx.recv().await.unwrap(), expected);
    }

    // Topic-based reads correctly partition events
    let trading = bus.store().read_topic("trading", 0, 100).unwrap();
    assert_eq!(trading.len(), 2, "should have 2 trading events");
    assert_eq!(trading[0].event_type, EventType::TradingOrderSubmitted);
    assert_eq!(trading[1].event_type, EventType::TradingOrderFilled);

    assert_eq!(bus.store().read_topic("research", 0, 100).unwrap().len(), 1);

    let feedback = bus.store().read_topic("feedback", 0, 100).unwrap();
    assert_eq!(feedback.len(), 1);
    assert_eq!(feedback[0].strategy_id.as_deref(), Some("strat-alpha"));
    assert_eq!(feedback[0].strategy_version, Some(3));

    assert_eq!(bus.store().read_topic("sentinel", 0, 100).unwrap().len(), 1);

    // Consumer offset tracking: two independent consumers
    let store = bus.store();
    store.set_offset("consumer-A", "trading", seq_fill).unwrap();
    store
        .set_offset("consumer-B", "trading", seq_trading)
        .unwrap();
    assert_eq!(store.get_offset("consumer-A", "trading").unwrap(), seq_fill);
    assert_eq!(
        store.get_offset("consumer-B", "trading").unwrap(),
        seq_trading
    );

    // Consumer B catches up (from_seq is inclusive, so +1 to skip processed)
    let catchup = store.read_topic("trading", seq_trading + 1, 100).unwrap();
    assert_eq!(
        catchup.len(),
        1,
        "consumer B should have 1 event to catch up"
    );
    assert_eq!(catchup[0].event_type, EventType::TradingOrderFilled);

    assert!(store.read_topic("nonexistent", 0, 100).unwrap().is_empty());
}

// ---------------------------------------------------------------------------
// 3a. Paper broker position netting logic
// ---------------------------------------------------------------------------

/// Validates that the paper broker correctly nets positions: same-side
/// accumulates, opposite-side reduces, and excess opposite-side flips.
#[tokio::test]
async fn paper_broker_position_netting() {
    let broker = PaperBroker::new(dec!(50_000));

    broker
        .push(&[StagedAction::builder()
            .action_type(ActionType::PlaceOrder)
            .contract_id("BTC-USD")
            .side(Side::Buy)
            .quantity(dec!(2.0))
            .order_type(OrderType::Market)
            .build()])
        .await
        .unwrap();

    let pos = broker.positions().await.unwrap();
    assert_eq!(pos[0].quantity, dec!(2.0));
    assert_eq!(pos[0].side, Side::Buy);

    // Same-side accumulates
    broker
        .push(&[StagedAction::builder()
            .action_type(ActionType::PlaceOrder)
            .contract_id("BTC-USD")
            .side(Side::Buy)
            .quantity(dec!(1.5))
            .order_type(OrderType::Market)
            .build()])
        .await
        .unwrap();
    assert_eq!(broker.positions().await.unwrap()[0].quantity, dec!(3.5));

    // Partial sell reduces
    broker
        .push(&[StagedAction::builder()
            .action_type(ActionType::PlaceOrder)
            .contract_id("BTC-USD")
            .side(Side::Sell)
            .quantity(dec!(1.0))
            .order_type(OrderType::Market)
            .build()])
        .await
        .unwrap();

    let pos = broker.positions().await.unwrap();
    assert_eq!(pos[0].quantity, dec!(2.5), "partial sell should reduce");
    assert_eq!(pos[0].side, Side::Buy, "side unchanged after partial sell");

    // Excess sell flips position
    broker
        .push(&[StagedAction::builder()
            .action_type(ActionType::PlaceOrder)
            .contract_id("BTC-USD")
            .side(Side::Sell)
            .quantity(dec!(4.0))
            .order_type(OrderType::Market)
            .build()])
        .await
        .unwrap();

    let pos = broker.positions().await.unwrap();
    assert_eq!(pos[0].quantity, dec!(1.5), "excess sell flips position");
    assert_eq!(pos[0].side, Side::Sell, "position should flip to Sell");
}

// ---------------------------------------------------------------------------
// 3b. Paper broker batch orders + guard pipeline integration
// ---------------------------------------------------------------------------

/// Validates batch order fills, execution history, and guard pipeline
/// allow/reject decisions based on symbol whitelisting.
#[tokio::test]
async fn paper_broker_batch_and_guard_pipeline() {
    let broker = PaperBroker::new(dec!(50_000));

    let batch = vec![
        StagedAction::builder()
            .action_type(ActionType::PlaceOrder)
            .contract_id("BTC-USD")
            .side(Side::Buy)
            .quantity(dec!(1.0))
            .order_type(OrderType::Market)
            .build(),
        StagedAction::builder()
            .action_type(ActionType::PlaceOrder)
            .contract_id("ETH-USD")
            .side(Side::Buy)
            .quantity(dec!(10.0))
            .order_type(OrderType::Market)
            .build(),
    ];
    let results = broker.push(&batch).await.unwrap();
    assert_eq!(results.len(), 2);
    assert!(results.iter().all(|r| r.status == OrderStatus::Filled));

    let executions = broker.sync_orders().await.unwrap();
    assert_eq!(executions.len(), 2);
    assert!(executions.iter().all(|e| e.price == dec!(50_000)));

    let account = broker.account_info().await.unwrap();
    assert_eq!(account.total_equity, Decimal::new(100_000, 0));
    assert_eq!(account.positions.len(), 2);

    // Guard pipeline: BTC-USD whitelisted -> allow
    let commit = TradingCommit::builder()
        .message("Buy BTC per momentum signal")
        .strategy_id("strat-momentum")
        .strategy_version(1)
        .actions(vec![
            StagedAction::builder()
                .action_type(ActionType::PlaceOrder)
                .contract_id("BTC-USD")
                .side(Side::Buy)
                .quantity(dec!(1.0))
                .order_type(OrderType::Market)
                .build(),
        ])
        .build();

    let allow_pipeline = GuardPipeline::new(vec![Box::new(SymbolWhitelist::new(vec![
        "BTC-USD".into(),
        "ETH-USD".into(),
    ]))]);
    assert!(matches!(
        allow_pipeline.run(&commit, &account).await,
        GuardResult::Allow
    ));

    // Only ETH-USD whitelisted -> reject BTC-USD commit
    let reject_pipeline =
        GuardPipeline::new(vec![Box::new(SymbolWhitelist::new(vec!["ETH-USD".into()]))]);
    assert!(matches!(
        reject_pipeline.run(&commit, &account).await,
        GuardResult::Reject { .. }
    ));
}

// ---------------------------------------------------------------------------
// 4. Cross-system: research acceptance -> event bus -> paper trade
// ---------------------------------------------------------------------------

/// Simulates the full handoff: research accepts a strategy, publishes a
/// promotion event to the bus, paper broker executes the first trade, and
/// the fill event is published back — validating the complete loop.
#[tokio::test]
async fn research_accept_to_event_to_paper_trade() {
    let dir = tempfile::tempdir().unwrap();

    // --- Research phase ---
    let trace = Trace::open(&dir.path().join("trace")).unwrap();

    let hyp = Hypothesis::builder()
        .text("Breakout on ETH when BTC vol is low")
        .reason("Cross-asset volatility regime signal")
        .build();
    trace.save_hypothesis(&hyp).unwrap();

    let exp = Experiment::builder()
        .hypothesis_id(hyp.id)
        .strategy_code("fn breakout_strategy() { /* ... */ }")
        .backtest_result(
            BacktestResult::builder()
                .pnl(dec!(1200.0))
                .sharpe_ratio(2.1)
                .max_drawdown(dec!(60.0))
                .win_rate(0.65)
                .trade_count(80)
                .build(),
        )
        .build();
    let fb = HypothesisFeedback::builder()
        .experiment_id(exp.id)
        .decision(true)
        .reason("Strong risk-adjusted returns, acceptable drawdown")
        .observations("Consistent performance across regimes")
        .build();
    trace.record(&exp, &fb, &DagSelection::NewRoot).unwrap();

    let sota = trace.get_sota().unwrap().expect("SOTA should exist");
    assert_eq!(sota.0.id, exp.id);

    // --- Event bus phase ---
    let bus = EventBus::open(&dir.path().join("events")).unwrap();
    let mut rx = bus.subscribe();

    let seq = bus
        .publish(
            &Event::builder()
                .event_type(EventType::ResearchStrategyCandidate)
                .source("research-loop")
                .correlation_id(exp.id.to_string())
                .strategy_id(exp.id.to_string())
                .strategy_version(1)
                .payload(json!({"hypothesis": hyp.text, "sharpe": 2.1}))
                .build(),
        )
        .unwrap();
    assert_eq!(rx.recv().await.unwrap(), seq);

    let candidates = bus.store().read_topic("research", 0, 10).unwrap();
    assert_eq!(candidates.len(), 1);
    assert_eq!(
        candidates[0].strategy_id.as_deref(),
        Some(exp.id.to_string()).as_deref()
    );

    // --- Paper trading phase ---
    let broker = PaperBroker::new(dec!(3200));
    let order_results = broker
        .push(&[StagedAction::builder()
            .action_type(ActionType::PlaceOrder)
            .contract_id("ETH-USD")
            .side(Side::Buy)
            .quantity(dec!(5.0))
            .order_type(OrderType::Market)
            .build()])
        .await
        .unwrap();
    assert_eq!(order_results[0].status, OrderStatus::Filled);

    // Publish fill event back to the bus
    bus.publish(
        &Event::builder()
            .event_type(EventType::TradingOrderFilled)
            .source("paper-broker")
            .correlation_id(order_results[0].order_id.clone())
            .strategy_id(exp.id.to_string())
            .strategy_version(1)
            .payload(json!({"contract": "ETH-USD", "price": 3200}))
            .build(),
    )
    .unwrap();

    let trading_events = bus.store().read_topic("trading", 0, 10).unwrap();
    assert_eq!(trading_events.len(), 1);
    assert_eq!(trading_events[0].event_type, EventType::TradingOrderFilled);

    let positions = broker.positions().await.unwrap();
    assert_eq!(positions.len(), 1);
    assert_eq!(positions[0].contract_id, "ETH-USD");
    assert_eq!(positions[0].quantity, dec!(5.0));
}

// ---------------------------------------------------------------------------
// 5. Strategy store artifact persistence
// ---------------------------------------------------------------------------

/// Validates that the strategy store persists and retrieves compiled artifacts
/// alongside metadata, with correct status filtering across lifecycle stages.
#[test]
fn strategy_store_lifecycle_with_artifacts() {
    let dir = tempfile::tempdir().unwrap();
    let db = sled::open(dir.path().join("db")).unwrap();
    let store = StrategyStore::open(&db, &dir.path().join("artifacts")).unwrap();

    let hyp_id = Uuid::new_v4();

    let s_compiled = ResearchStrategy::builder()
        .hypothesis_id(hyp_id)
        .source_code("fn compiled() {}")
        .build();
    let s_accepted = ResearchStrategy::builder()
        .hypothesis_id(hyp_id)
        .source_code("fn accepted() {}")
        .build();
    let s_promoted = ResearchStrategy::builder()
        .hypothesis_id(hyp_id)
        .source_code("fn promoted() {}")
        .build();

    store.save(&s_compiled).unwrap();
    store.save(&s_accepted).unwrap();
    store.save(&s_promoted).unwrap();

    store
        .update_status(s_accepted.id, ResearchStrategyStatus::Accepted)
        .unwrap();
    store
        .update_status(s_promoted.id, ResearchStrategyStatus::Accepted)
        .unwrap();
    store
        .update_status(s_promoted.id, ResearchStrategyStatus::Promoted)
        .unwrap();

    assert_eq!(
        store
            .list(Some(ResearchStrategyStatus::Compiled))
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        store
            .list(Some(ResearchStrategyStatus::Accepted))
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        store
            .list(Some(ResearchStrategyStatus::Promoted))
            .unwrap()
            .len(),
        1
    );
    assert_eq!(store.list(None).unwrap().len(), 3);

    // Artifact persistence
    let fake_wasm = b"\x00asm\x01\x00\x00\x00fake wasm module bytes";
    store.save_artifact(s_promoted.id, fake_wasm).unwrap();
    let loaded = store.load_artifact(s_promoted.id).unwrap();
    assert_eq!(loaded, fake_wasm, "artifact bytes must round-trip exactly");

    // Nonexistent artifact returns an error, not a panic
    assert!(store.load_artifact(Uuid::new_v4()).is_err());
}
