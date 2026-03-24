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

