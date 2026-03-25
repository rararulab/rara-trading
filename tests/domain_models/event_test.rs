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
