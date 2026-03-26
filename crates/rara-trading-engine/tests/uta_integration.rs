//! Integration test: full UTA lifecycle with `PaperBroker`.
//!
//! Covers the stage → commit → push pipeline, reject workflow,
//! health tracking, and `AccountManager` multi-account aggregation.

use rust_decimal::Decimal;
use rust_decimal_macros::dec;

use rara_domain::contract::{Contract, SecType};
use rara_domain::trading::operation::OperationOrderType;
use rara_domain::trading::Side;

use rara_trading_engine::brokers::paper::PaperBroker;
use rara_trading_engine::health::BrokerHealth;
use rara_trading_engine::uta::UnifiedTradingAccount;

fn btc_contract() -> Contract {
    Contract::builder()
        .exchange("paper")
        .symbol("BTCUSDT")
        .sec_type(SecType::CryptoSpot)
        .currency("USDT")
        .build()
}

fn eth_contract() -> Contract {
    Contract::builder()
        .exchange("paper")
        .symbol("ETHUSDT")
        .sec_type(SecType::CryptoSpot)
        .currency("USDT")
        .build()
}

fn make_uta(id: &str, fill_price: Decimal) -> UnifiedTradingAccount {
    UnifiedTradingAccount::new(id, id, Box::new(PaperBroker::new(fill_price)))
}

// ── Test 1: full lifecycle ─────────────────────────────────────────────

#[tokio::test]
async fn full_lifecycle_stage_commit_push_log() {
    let uta = make_uta("paper-1", dec!(50000));

    // Stage BTC market order
    let add1 = uta
        .stage_place_order(
            btc_contract(),
            Side::Buy,
            OperationOrderType::Market,
            dec!(0.5),
            None,
        )
        .await;
    assert!(add1.staged);

    // Stage ETH limit order
    let add2 = uta
        .stage_place_order(
            eth_contract(),
            Side::Buy,
            OperationOrderType::Limit,
            dec!(10),
            Some(dec!(3500)),
        )
        .await;
    assert!(add2.staged);

    // Status should show 2 staged operations, no commits yet
    let status = uta.status().await;
    assert_eq!(status.staged.len(), 2);
    assert_eq!(status.commit_count, 0);

    // Commit
    let commit = uta
        .commit("buy BTC + ETH")
        .await
        .expect("commit should succeed with staged ops");
    assert!(commit.prepared);
    assert_eq!(commit.operation_count, 2);

    // Push — dispatches to PaperBroker
    let push = uta.push().await.expect("push should succeed");
    assert_eq!(push.operation_count, 2);
    assert_eq!(push.submitted.len(), 2, "both orders should be accepted");
    assert!(push.rejected.is_empty());

    // Log should contain exactly 1 commit with 2 operations
    let log = uta.log(10, None).await;
    assert_eq!(log.len(), 1);
    assert_eq!(log[0].operations.len(), 2);
    assert!(log[0].message.contains("buy BTC + ETH"));

    // Show by hash should return the same commit
    let shown = uta.show(&push.hash).await.expect("commit should be retrievable by hash");
    assert_eq!(shown.operations.len(), 2);

    // Export should contain 1 commit
    let export = uta.export_git_state().await;
    assert_eq!(export.commits.len(), 1);
    assert_eq!(export.head, Some(push.hash));
}

// ── Test 2: AccountManager multi-account ───────────────────────────────

#[tokio::test]
async fn account_manager_multi_account() {
    use rara_trading_engine::account_manager::AccountManager;

    let uta1 = make_uta("acct-btc", dec!(50000));
    let uta2 = make_uta("acct-eth", dec!(3500));

    let mut manager = AccountManager::new();
    manager.add(uta1);
    manager.add(uta2);

    assert_eq!(manager.size(), 2);

    // resolve_one should find by id
    let found = manager.resolve_one("acct-btc").expect("should find acct-btc");
    assert_eq!(found.id, "acct-btc");

    // resolve_one should fail on unknown
    assert!(manager.resolve_one("nope").is_err());

    // aggregated_equity should reflect 2 accounts
    let equity = manager.aggregated_equity().await;
    assert_eq!(equity.accounts.len(), 2);
    assert!(equity.total_equity > Decimal::ZERO);
}

// ── Test 3: reject workflow ────────────────────────────────────────────

#[tokio::test]
async fn reject_workflow() {
    use rara_domain::trading::operation::OperationStatus;

    let uta = make_uta("paper-reject", dec!(50000));

    // Stage an order
    uta.stage_place_order(
        btc_contract(),
        Side::Buy,
        OperationOrderType::Market,
        dec!(1),
        None,
    )
    .await;

    // Commit but reject instead of pushing
    uta.commit("risky trade").await.expect("should commit");
    let reject = uta.reject("risk limit exceeded").await.expect("reject should succeed");

    assert_eq!(reject.operation_count, 1);
    assert!(reject.message.contains("REJECTED"), "message should indicate rejection");
    assert!(reject.message.contains("risk limit exceeded"), "message should include reason");

    // Verify no positions were opened on the broker
    let positions = uta.get_positions().await.expect("should get positions");
    assert!(positions.is_empty(), "rejected orders must not create positions");

    // Log should show 1 commit with UserRejected status
    let log = uta.log(10, None).await;
    assert_eq!(log.len(), 1);
    assert_eq!(log[0].operations.len(), 1);
    assert_eq!(
        log[0].operations[0].status,
        OperationStatus::UserRejected,
        "rejected operation must have UserRejected status"
    );
}

// ── Test 4: health tracking with broker ────────────────────────────────

#[tokio::test]
async fn health_tracking_with_broker() {
    let uta = make_uta("paper-health", dec!(50000));

    // New UTA should start healthy
    assert_eq!(uta.health().await, BrokerHealth::Healthy);
    assert!(!uta.is_disabled().await);

    // Successful broker query should keep it healthy
    let _account = uta.get_account().await.expect("account_info should succeed");
    assert_eq!(uta.health().await, BrokerHealth::Healthy);

    // A full stage-commit-push cycle also keeps health healthy
    uta.stage_place_order(
        btc_contract(),
        Side::Buy,
        OperationOrderType::Market,
        dec!(0.1),
        None,
    )
    .await;
    uta.commit("health check trade").await;
    uta.push().await.expect("push should succeed");

    let info = uta.health_info().await;
    assert_eq!(info.status, BrokerHealth::Healthy);
    assert_eq!(info.consecutive_failures, 0);
    assert!(info.last_success_at.is_some(), "should have recorded a success timestamp");
}
