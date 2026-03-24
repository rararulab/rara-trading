use rust_decimal_macros::dec;

use rara_trading::domain::trading::{ActionType, OrderType, Side, StagedAction, TradingCommit};

#[test]
fn trading_commit_hash_is_8_chars() {
    let action = StagedAction::builder()
        .action_type(ActionType::PlaceOrder)
        .contract_id("binance-BTCUSDT")
        .side(Side::Buy)
        .quantity(dec!(0.5))
        .order_type(OrderType::Market)
        .build();

    let commit = TradingCommit::builder()
        .message("open long BTC")
        .actions(vec![action])
        .strategy_id("strat-001")
        .strategy_version(1)
        .build();

    assert_eq!(commit.hash().len(), 8);
}
