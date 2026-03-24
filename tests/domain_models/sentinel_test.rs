use rara_trading::domain::sentinel::{SentinelSignal, Severity, SignalSource, SignalType};

#[test]
fn critical_signal_should_block_trading() {
    let signal = SentinelSignal::builder()
        .signal_type(SignalType::BlackSwan)
        .severity(Severity::Critical)
        .source(SignalSource::PriceAction)
        .affected_contracts(vec!["binance-BTCUSDT".to_string()])
        .summary("Flash crash detected")
        .raw_data(serde_json::json!({}))
        .build();

    assert!(signal.should_block_trading());
}

#[test]
fn info_signal_should_not_block() {
    let signal = SentinelSignal::builder()
        .signal_type(SignalType::SentimentShift)
        .severity(Severity::Info)
        .source(SignalSource::SocialMedia)
        .affected_contracts(vec![])
        .summary("Mild positive sentiment shift")
        .raw_data(serde_json::json!({}))
        .build();

    assert!(!signal.should_block_trading());
}
