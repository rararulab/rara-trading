use rara_trading::domain::contract::SecType;
use rara_trading::domain::strategy::{
    ContractFilter, RiskProfile, Strategy, StrategyStatus, StrategyType,
};
use rust_decimal::Decimal;

#[test]
fn strategy_builder_works() {
    let s = Strategy::builder()
        .id("strat-001")
        .version(1)
        .name("BTC Momentum")
        .description("Trend following on BTC")
        .code("fn run() {}")
        .strategy_type(StrategyType::Directional)
        .applicable_contracts(vec![ContractFilter::BySecType(SecType::CryptoSpot)])
        .parameters(serde_json::json!({"lookback": 20}))
        .status(StrategyStatus::Candidate)
        .build();

    assert_eq!(s.id(), "strat-001");
    assert_eq!(s.version(), 1);
    assert_eq!(s.name(), "BTC Momentum");
    assert_eq!(s.status(), StrategyStatus::Candidate);
    assert_eq!(s.strategy_type(), StrategyType::Directional);
    assert_eq!(s.applicable_contracts().len(), 1);
}

#[test]
fn risk_profile_crypto_perp_requires_stop_loss() {
    let rp = RiskProfile::crypto_perp_default();
    assert!(rp.require_stop_loss());
    assert!(rp.funding_rate_check());
    assert_eq!(rp.max_leverage(), Decimal::from(5));
}

#[test]
fn risk_profile_spot_no_leverage() {
    let rp = RiskProfile::crypto_spot_default();
    assert_eq!(rp.max_leverage(), Decimal::from(1));
    assert!(!rp.require_stop_loss());
    assert!(!rp.funding_rate_check());
}
