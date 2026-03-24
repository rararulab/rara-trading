use rara_trading::domain::contract::{Contract, SecType};

#[test]
fn contract_id_format() {
    let c = Contract::builder()
        .exchange("binance")
        .symbol("BTCUSDT")
        .sec_type(SecType::CryptoSpot)
        .currency("USDT")
        .build();
    assert_eq!(c.id(), "binance-BTCUSDT");
}

#[test]
fn sec_type_distinguishes_spot_and_perp() {
    assert_ne!(SecType::CryptoSpot, SecType::CryptoPerp);
}

#[test]
fn contract_getters() {
    let c = Contract::builder()
        .exchange("coinbase")
        .symbol("ETHUSDC")
        .sec_type(SecType::CryptoSpot)
        .currency("USDC")
        .build();
    assert_eq!(c.exchange(), "coinbase");
    assert_eq!(c.symbol(), "ETHUSDC");
    assert_eq!(c.sec_type(), SecType::CryptoSpot);
    assert_eq!(c.currency(), "USDC");
}
