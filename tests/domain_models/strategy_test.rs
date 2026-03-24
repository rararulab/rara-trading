use rara_trading::domain::strategy::RiskProfile;
use rust_decimal::Decimal;

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
