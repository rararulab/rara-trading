use rara_trading::domain::event::Event;

#[test]
fn event_topic_extraction() {
    let e = Event::builder()
        .event_type("trading.order.filled")
        .source("order-engine")
        .correlation_id("corr-123")
        .payload(serde_json::json!({"qty": 1}))
        .build();
    assert_eq!(e.topic(), "trading");
}

#[test]
fn event_serialization_roundtrip() {
    let e = Event::builder()
        .event_type("market.tick")
        .source("feed")
        .correlation_id("corr-456")
        .payload(serde_json::json!({"price": "50000"}))
        .build();

    let json = serde_json::to_string(&e).expect("serialize");
    let e2: Event = serde_json::from_str(&json).expect("deserialize");

    assert_eq!(e.event_id(), e2.event_id());
    assert_eq!(e.event_type(), e2.event_type());
    assert_eq!(e.source(), e2.source());
    assert_eq!(e.correlation_id(), e2.correlation_id());
}
