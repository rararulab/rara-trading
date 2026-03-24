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

#[test]
fn staged_action_limit_order_has_price() {
    let action = StagedAction::builder()
        .action_type(ActionType::PlaceOrder)
        .contract_id("binance-BTCUSDT")
        .side(Side::Buy)
        .quantity(dec!(1.0))
        .order_type(OrderType::Limit)
        .limit_price(dec!(49000))
        .build();

    assert_eq!(action.limit_price(), Some(dec!(49000)));
    assert_eq!(action.order_type(), OrderType::Limit);
}
