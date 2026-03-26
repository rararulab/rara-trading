//! End-to-end smoke tests validating the research -> paper trading -> feedback
//! loop works across crate boundaries.
//!
//! These tests exercise real implementations (sled stores, paper broker, guard
//! pipeline) with temporary directories — no mocks.

use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde_json::json;
use uuid::Uuid;

use rara_domain::event::{Event, EventType};
use rara_domain::research::{
    BacktestResult, Experiment, Hypothesis, HypothesisFeedback, ResearchStrategy,
    ResearchStrategyStatus,
};
use rara_domain::trading::{ActionType, OrderType, Side, StagedAction, TradingCommit};
use rara_event_bus::bus::EventBus;
use rara_research::strategy_store::StrategyStore;
use rara_research::trace::{DagSelection, Trace};
use rara_trading_engine::broker::{Broker, OrderStatus};
use rara_trading_engine::brokers::paper::PaperBroker;
use rara_trading_engine::guard_pipeline::GuardPipeline;
use rara_trading_engine::guards::symbol_whitelist::SymbolWhitelist;
use rara_trading_engine::guards::GuardResult;

// ---------------------------------------------------------------------------
// 1a. Research trace DAG: hypothesis lineage + SOTA selection
// ---------------------------------------------------------------------------

/// Helper: builds a shared trace with 3 experiments (v1 accepted, v2 accepted
/// with higher Sharpe, rejected overfit) and returns key handles for assertions.
struct TraceFixture {
    dir: tempfile::TempDir,
    trace: Trace,
    root_hyp: Hypothesis,
    refined_hyp: Hypothesis,
    exp_v1: Experiment,
    exp_v2: Experiment,
    exp_rejected: Experiment,
    idx0: u64,
    idx1: u64,
}

fn build_trace_fixture() -> TraceFixture {
    let dir = tempfile::tempdir().unwrap();
    let trace = Trace::open(&dir.path().join("trace")).unwrap();

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

    // Experiment v1: mediocre Sharpe, accepted
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
        .reason("Acceptable Sharpe but high drawdown — needs volatility filter")
        .observations("Drawdown spikes during high-vol periods")
        .build();
    let idx0 = trace.record(&exp_v1, &fb_v1, &DagSelection::NewRoot).unwrap();

    // Experiment v2: better Sharpe, accepted
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
    let idx1 = trace.record(&exp_v2, &fb_v2, &DagSelection::Latest).unwrap();

    // Rejected experiment: high Sharpe but overfitting
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

    TraceFixture {
        dir,
        trace,
        root_hyp,
        refined_hyp,
        exp_v1,
        exp_v2,
        exp_rejected,
        idx0,
        idx1,
    }
}

/// Validates hypothesis lineage, DAG structure, and SOTA selection logic:
/// the best accepted experiment (highest Sharpe) wins over rejected ones.
#[test]
fn research_trace_dag_and_sota_selection() {
    let f = build_trace_fixture();

    // Verify ancestor chain correctly links hypotheses
    let chain = f.trace.ancestor_chain(f.refined_hyp.id).unwrap();
    assert_eq!(chain.len(), 2, "refined hypothesis should have root as ancestor");
    assert_eq!(chain[0].id, f.refined_hyp.id);
    assert_eq!(chain[1].id, f.root_hyp.id);

    // SOTA should pick exp_v2 (highest Sharpe among accepted)
    let sota = f.trace.get_sota().unwrap().expect("should have a SOTA");
    assert_eq!(sota.0.id, f.exp_v2.id, "SOTA must be the v2 experiment with Sharpe 1.9");
    assert!(sota.1.decision, "SOTA feedback must be accepted");

    // DAG ancestor walk from idx1 should reach idx0
    let ancestors = f.trace.ancestors(f.idx1).unwrap();
    assert_eq!(ancestors.len(), 2);
    assert_eq!(ancestors[0].0.id, f.exp_v2.id);
    assert_eq!(ancestors[1].0.id, f.exp_v1.id);

    // DAG children of root should include both v2 (via Latest) and rejected
    let children = f.trace.children(f.idx0).unwrap();
    assert_eq!(children.len(), 2, "root node should have 2 children");

    // Trace prompt formatting includes both iterations
    let prompt = f.trace.format_for_prompt(10).unwrap();
    assert!(prompt.contains("accepted"), "prompt should mention accepted experiments");
    assert!(prompt.contains("rejected"), "prompt should mention rejected experiments");

    // list_recent returns newest first
    let recent = f.trace.list_recent(10).unwrap();
    assert_eq!(recent.len(), 3);
    assert_eq!(
        recent[0].1.id, f.exp_rejected.id,
        "most recent experiment should be listed first"
    );
}

// ---------------------------------------------------------------------------
// 1b. Strategy store promotion lifecycle
// ---------------------------------------------------------------------------

/// Validates that the strategy store correctly tracks promotion status
/// transitions (Compiled -> Accepted -> Promoted) and filters by status.
#[test]
fn strategy_store_promotion_lifecycle() {
    let f = build_trace_fixture();
    let db = sled::open(f.dir.path().join("strategy_db")).unwrap();
    let store = StrategyStore::open(&db, &f.dir.path().join("artifacts")).unwrap();

    // Promote the SOTA strategy through the strategy store
    let promoted = ResearchStrategy::builder()
        .hypothesis_id(f.refined_hyp.id)
        .source_code(&f.exp_v2.strategy_code)
        .build();
    store.save(&promoted).unwrap();

    // Verify lifecycle: Compiled -> Accepted -> Promoted
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

    // Strategy store filtering works correctly
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

    // Publish events across all four topic domains
    let trading_event = Event::builder()
        .event_type(EventType::TradingOrderSubmitted)
        .source("trading-engine")
        .correlation_id("trade-001")
        .payload(json!({"symbol": "BTC-USD", "side": "buy", "qty": 0.5}))
        .build();

    let research_event = Event::builder()
        .event_type(EventType::ResearchExperimentCompleted)
        .source("research-loop")
        .correlation_id("exp-001")
        .payload(json!({"sharpe": 1.5, "accepted": true}))
        .build();

    let feedback_event = Event::builder()
        .event_type(EventType::FeedbackStrategyPromote)
        .source("feedback-engine")
        .correlation_id("fb-001")
        .strategy_id("strat-alpha".into())
        .strategy_version(3)
        .payload(json!({"action": "promote", "reason": "sustained alpha"}))
        .build();

    let sentinel_event = Event::builder()
        .event_type(EventType::SentinelSignalDetected)
        .source("sentinel")
        .correlation_id("sig-001")
        .payload(json!({"signal": "volatility_spike", "severity": "high"}))
        .build();

    let fill_event = Event::builder()
        .event_type(EventType::TradingOrderFilled)
        .source("trading-engine")
        .correlation_id("trade-002")
        .payload(json!({"symbol": "ETH-USD", "price": 3200}))
        .build();

    // Publish all events and collect sequence numbers
    let seq_trading = bus.publish(&trading_event).unwrap();
    let seq_research = bus.publish(&research_event).unwrap();
    let seq_feedback = bus.publish(&feedback_event).unwrap();
    let seq_sentinel = bus.publish(&sentinel_event).unwrap();
    let seq_fill = bus.publish(&fill_event).unwrap();

    // Sequence numbers must be monotonically increasing
    assert!(seq_research > seq_trading);
    assert!(seq_feedback > seq_research);
    assert!(seq_sentinel > seq_feedback);
    assert!(seq_fill > seq_sentinel);

    // Broadcast subscriber receives all 5 sequence numbers in order
    for expected_seq in [seq_trading, seq_research, seq_feedback, seq_sentinel, seq_fill] {
        let received = rx.recv().await.unwrap();
        assert_eq!(received, expected_seq);
    }

    // Topic-based reads correctly partition events
    let trading_events = bus.store().read_topic("trading", 0, 100).unwrap();
    assert_eq!(trading_events.len(), 2, "should have 2 trading events");
    assert_eq!(trading_events[0].event_type, EventType::TradingOrderSubmitted);
    assert_eq!(trading_events[1].event_type, EventType::TradingOrderFilled);

    let research_events = bus.store().read_topic("research", 0, 100).unwrap();
    assert_eq!(research_events.len(), 1);
    assert_eq!(
        research_events[0].event_type,
        EventType::ResearchExperimentCompleted
    );

    let feedback_events = bus.store().read_topic("feedback", 0, 100).unwrap();
    assert_eq!(feedback_events.len(), 1);
    assert_eq!(feedback_events[0].event_type, EventType::FeedbackStrategyPromote);
    // Verify strategy metadata is preserved through serialization
    assert_eq!(
        feedback_events[0].strategy_id.as_deref(),
        Some("strat-alpha")
    );
    assert_eq!(feedback_events[0].strategy_version, Some(3));

    let sentinel_events = bus.store().read_topic("sentinel", 0, 100).unwrap();
    assert_eq!(sentinel_events.len(), 1);

    // Consumer offset tracking: two independent consumers on "trading" topic
    let store = bus.store();

    // Consumer A processes both trading events
    store.set_offset("consumer-A", "trading", seq_fill).unwrap();
    assert_eq!(store.get_offset("consumer-A", "trading").unwrap(), seq_fill);

    // Consumer B only processed the first trading event
    store
        .set_offset("consumer-B", "trading", seq_trading)
        .unwrap();
    assert_eq!(
        store.get_offset("consumer-B", "trading").unwrap(),
        seq_trading
    );

    // Consumer B catches up: reads trading events after its offset
    // (from_seq is inclusive, so use offset + 1 to skip already-processed)
    let catchup = store.read_topic("trading", seq_trading + 1, 100).unwrap();
    assert_eq!(catchup.len(), 1, "consumer B should have 1 event to catch up on");
    assert_eq!(catchup[0].event_type, EventType::TradingOrderFilled);

    // Nonexistent topic returns empty
    let empty = store.read_topic("nonexistent", 0, 100).unwrap();
    assert!(empty.is_empty());
}

// ---------------------------------------------------------------------------
// 3a. Paper broker position netting logic
// ---------------------------------------------------------------------------

/// Validates the paper broker fills orders and tracks positions with correct
/// netting logic: accumulation, partial close, and side flip.
#[tokio::test]
async fn paper_broker_position_netting() {
    let broker = PaperBroker::new(dec!(50_000));

    // Submit a buy order for BTC-USD
    let buy_actions = vec![StagedAction::builder()
        .action_type(ActionType::PlaceOrder)
        .contract_id("BTC-USD")
        .side(Side::Buy)
        .quantity(dec!(2.0))
        .order_type(OrderType::Market)
        .build()];

    let results = broker.push(&buy_actions).await.unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].status, OrderStatus::Filled);
    assert_eq!(results[0].contract_id, "BTC-USD");

    // Position should reflect the buy
    let positions = broker.positions().await.unwrap();
    assert_eq!(positions.len(), 1);
    assert_eq!(positions[0].contract_id, "BTC-USD");
    assert_eq!(positions[0].side, Side::Buy);
    assert_eq!(positions[0].quantity, dec!(2.0));
    assert_eq!(positions[0].avg_entry_price, dec!(50_000));

    // Submit another buy to increase position
    let more_buy = vec![StagedAction::builder()
        .action_type(ActionType::PlaceOrder)
        .contract_id("BTC-USD")
        .side(Side::Buy)
        .quantity(dec!(1.5))
        .order_type(OrderType::Market)
        .build()];
    broker.push(&more_buy).await.unwrap();

    let positions = broker.positions().await.unwrap();
    assert_eq!(positions[0].quantity, dec!(3.5), "same-side buys should accumulate");

    // Partial sell reduces position but keeps side as Buy
    let partial_sell = vec![StagedAction::builder()
        .action_type(ActionType::PlaceOrder)
        .contract_id("BTC-USD")
        .side(Side::Sell)
        .quantity(dec!(1.0))
        .order_type(OrderType::Market)
        .build()];
    broker.push(&partial_sell).await.unwrap();

    let positions = broker.positions().await.unwrap();
    assert_eq!(positions[0].quantity, dec!(2.5), "partial sell should reduce position");
    assert_eq!(positions[0].side, Side::Buy, "side should remain Buy after partial sell");

    // Sell more than remaining flips the position to Sell
    let flip_sell = vec![StagedAction::builder()
        .action_type(ActionType::PlaceOrder)
        .contract_id("BTC-USD")
        .side(Side::Sell)
        .quantity(dec!(4.0))
        .order_type(OrderType::Market)
        .build()];
    broker.push(&flip_sell).await.unwrap();

    let positions = broker.positions().await.unwrap();
    assert_eq!(positions[0].quantity, dec!(1.5), "excess sell should flip position");
    assert_eq!(positions[0].side, Side::Sell, "position should flip to Sell");
}

// ---------------------------------------------------------------------------
// 3b. Paper broker batch orders + guard pipeline integration
// ---------------------------------------------------------------------------

/// Validates multi-contract batch orders, execution history, and guard pipeline
/// accept/reject logic based on symbol whitelist.
#[tokio::test]
async fn paper_broker_batch_and_guard_pipeline() {
    let broker = PaperBroker::new(dec!(50_000));

    // Multi-contract batch order
    let batch = vec![
        StagedAction::builder()
            .action_type(ActionType::PlaceOrder)
            .contract_id("BTC-USD")
            .side(Side::Buy)
            .quantity(dec!(2.0))
            .order_type(OrderType::Market)
            .build(),
        StagedAction::builder()
            .action_type(ActionType::PlaceOrder)
            .contract_id("ETH-USD")
            .side(Side::Buy)
            .quantity(dec!(10.0))
            .order_type(OrderType::Market)
            .build(),
        StagedAction::builder()
            .action_type(ActionType::PlaceOrder)
            .contract_id("SOL-USD")
            .side(Side::Sell)
            .quantity(dec!(100.0))
            .order_type(OrderType::Market)
            .build(),
    ];
    let batch_results = broker.push(&batch).await.unwrap();
    assert_eq!(batch_results.len(), 3);
    assert!(batch_results.iter().all(|r| r.status == OrderStatus::Filled));

    // Execution history should contain all orders
    let executions = broker.sync_orders().await.unwrap();
    assert_eq!(executions.len(), 3, "should have 3 total execution reports");
    assert!(
        executions.iter().all(|e| e.price == dec!(50_000)),
        "all fills should be at the paper broker's fixed price"
    );

    // Account info reports positions
    let account = broker.account_info().await.unwrap();
    assert_eq!(account.total_equity, Decimal::new(100_000, 0));
    assert_eq!(account.positions.len(), 3, "should track BTC, ETH, SOL positions");

    // Build a commit that the guard pipeline will evaluate
    let commit = TradingCommit::builder()
        .message("Buy BTC per momentum signal")
        .strategy_id("strat-momentum")
        .strategy_version(1)
        .actions(vec![StagedAction::builder()
            .action_type(ActionType::PlaceOrder)
            .contract_id("BTC-USD")
            .side(Side::Buy)
            .quantity(dec!(1.0))
            .order_type(OrderType::Market)
            .build()])
        .build();

    // Pipeline with BTC-USD whitelisted: should allow
    let allow_pipeline = GuardPipeline::new(vec![Box::new(SymbolWhitelist::new(vec![
        "BTC-USD".to_string(),
        "ETH-USD".to_string(),
    ]))]);
    let result = allow_pipeline.run(&commit, &account).await;
    assert!(
        matches!(result, GuardResult::Allow),
        "whitelisted symbol should pass guard"
    );

    // Pipeline with only ETH-USD whitelisted: should reject BTC-USD commit
    let reject_pipeline = GuardPipeline::new(vec![Box::new(SymbolWhitelist::new(vec![
        "ETH-USD".to_string(),
    ]))]);
    let result = reject_pipeline.run(&commit, &account).await;
    assert!(
        matches!(result, GuardResult::Reject { .. }),
        "non-whitelisted symbol should be rejected"
    );
}

// ---------------------------------------------------------------------------
// 4. Cross-system: research acceptance -> event bus notification -> paper trade
// ---------------------------------------------------------------------------

/// Simulates the handoff between systems: research produces an accepted
/// strategy, publishes a promotion event, and the paper broker executes the
/// strategy's first trade — validating the full loop without external deps.
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

    // --- Event bus phase: publish promotion event ---
    let bus = EventBus::open(&dir.path().join("events")).unwrap();
    let mut rx = bus.subscribe();

    let promote_event = Event::builder()
        .event_type(EventType::ResearchStrategyCandidate)
        .source("research-loop")
        .correlation_id(exp.id.to_string())
        .strategy_id(exp.id.to_string())
        .strategy_version(1)
        .payload(json!({
            "hypothesis": hyp.text,
            "sharpe": 2.1,
            "pnl": 1200.0,
        }))
        .build();
    let seq = bus.publish(&promote_event).unwrap();

    // Subscriber receives the notification
    let notified_seq = rx.recv().await.unwrap();
    assert_eq!(notified_seq, seq);

    // Event can be retrieved by topic
    let candidates = bus.store().read_topic("research", 0, 10).unwrap();
    assert_eq!(candidates.len(), 1);
    assert_eq!(
        candidates[0].strategy_id.as_deref(),
        Some(exp.id.to_string()).as_deref()
    );

    // --- Paper trading phase: execute the strategy's first signal ---
    let broker = PaperBroker::new(dec!(3200));

    let actions = vec![StagedAction::builder()
        .action_type(ActionType::PlaceOrder)
        .contract_id("ETH-USD")
        .side(Side::Buy)
        .quantity(dec!(5.0))
        .order_type(OrderType::Market)
        .build()];
    let order_results = broker.push(&actions).await.unwrap();
    assert_eq!(order_results[0].status, OrderStatus::Filled);

    // Publish the fill event back to the bus
    let fill_event = Event::builder()
        .event_type(EventType::TradingOrderFilled)
        .source("paper-broker")
        .correlation_id(order_results[0].order_id.clone())
        .strategy_id(exp.id.to_string())
        .strategy_version(1)
        .payload(json!({
            "contract": "ETH-USD",
            "side": "buy",
            "qty": 5.0,
            "price": 3200,
        }))
        .build();
    bus.publish(&fill_event).unwrap();

    // Trading topic now has the fill
    let trading_events = bus.store().read_topic("trading", 0, 10).unwrap();
    assert_eq!(trading_events.len(), 1);
    assert_eq!(trading_events[0].event_type, EventType::TradingOrderFilled);

    // Verify the paper broker position matches
    let positions = broker.positions().await.unwrap();
    assert_eq!(positions.len(), 1);
    assert_eq!(positions[0].contract_id, "ETH-USD");
    assert_eq!(positions[0].quantity, dec!(5.0));
    assert_eq!(positions[0].avg_entry_price, dec!(3200));
}

// ---------------------------------------------------------------------------
// 5. Strategy store artifact persistence
// ---------------------------------------------------------------------------

/// Validates that the strategy store correctly persists and retrieves compiled
/// artifacts alongside metadata, and that status filtering works across
/// multiple strategies in different lifecycle stages.
#[test]
fn strategy_store_lifecycle_with_artifacts() {
    let dir = tempfile::tempdir().unwrap();
    let db = sled::open(dir.path().join("db")).unwrap();
    let store = StrategyStore::open(&db, &dir.path().join("artifacts")).unwrap();

    let hyp_id = Uuid::new_v4();

    // Create strategies at different lifecycle stages
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

    // Verify filtering by each status
    assert_eq!(
        store.list(Some(ResearchStrategyStatus::Compiled)).unwrap().len(),
        1
    );
    assert_eq!(
        store.list(Some(ResearchStrategyStatus::Accepted)).unwrap().len(),
        1
    );
    assert_eq!(
        store.list(Some(ResearchStrategyStatus::Promoted)).unwrap().len(),
        1
    );
    assert_eq!(store.list(None).unwrap().len(), 3);

    // Save and retrieve an artifact
    let fake_wasm = b"\x00asm\x01\x00\x00\x00fake wasm module bytes";
    store.save_artifact(s_promoted.id, fake_wasm).unwrap();

    let loaded = store.load_artifact(s_promoted.id).unwrap();
    assert_eq!(loaded, fake_wasm, "artifact bytes must round-trip exactly");

    // Nonexistent artifact returns an error, not a panic
    let missing = store.load_artifact(Uuid::new_v4());
    assert!(missing.is_err(), "loading nonexistent artifact should fail");
}
