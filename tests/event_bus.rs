//! Integration tests for the event bus module.

use rara_trading::event_bus::bus::EventBus;
use rara_trading::domain::event::Event;
use serde_json::json;

#[tokio::test]
async fn end_to_end_publish_subscribe_catch_up() {
    let dir = tempfile::tempdir().unwrap();
    let bus = EventBus::open(dir.path()).unwrap();

    // 1. Subscribe before publishing
    let mut rx = bus.subscribe();

    // 2. Publish research event (strategy_id set)
    let research = Event::builder()
        .event_type("research.hypothesis.created")
        .source("research-engine")
        .correlation_id("corr-research-1")
        .strategy_id("strat-alpha".to_string())
        .payload(json!({"hypothesis": "momentum works"}))
        .build();
    let seq1 = bus.publish(&research).unwrap();

    // 3. Publish trading event (strategy_id + strategy_version set)
    let trading = Event::builder()
        .event_type("trading.order.placed")
        .source("trading-engine")
        .correlation_id("corr-trading-1")
        .strategy_id("strat-alpha".to_string())
        .strategy_version(3)
        .payload(json!({"pair": "BTC/USD", "side": "buy"}))
        .build();
    let seq2 = bus.publish(&trading).unwrap();

    // 4. Verify online subscriber gets both seq numbers
    let recv1 = rx.recv().await.unwrap();
    let recv2 = rx.recv().await.unwrap();
    assert_eq!(recv1, seq1);
    assert_eq!(recv2, seq2);

    // 5. Verify read_topic("research", 0, 10) -> 1 event
    let research_events = bus.store().read_topic("research", 0, 10).unwrap();
    assert_eq!(research_events.len(), 1);
    assert_eq!(
        research_events[0].event_type,
        "research.hypothesis.created"
    );

    // 6. Verify read_topic("trading", 0, 10) -> 1 event
    let trading_events = bus.store().read_topic("trading", 0, 10).unwrap();
    assert_eq!(trading_events.len(), 1);
    assert_eq!(trading_events[0].event_type, "trading.order.placed");

    // 7. Verify consumer offset set/get works
    bus.store()
        .set_offset("worker-1", "trading", seq2)
        .unwrap();
    let offset = bus.store().get_offset("worker-1", "trading").unwrap();
    assert_eq!(offset, seq2);

    // Unset consumer defaults to 0
    let default_offset = bus.store().get_offset("worker-2", "trading").unwrap();
    assert_eq!(default_offset, 0);
}
