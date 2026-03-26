//! Unified Trading Account — wraps a [`Broker`] with git-style operation
//! history ([`TradingGit`]) and connection health tracking ([`HealthTracker`]).
//!
//! The UTA bridges domain operations to broker calls by implementing
//! [`OperationDispatcher`] and [`StateProvider`] via internal adapter structs.

use async_trait::async_trait;
use rust_decimal::Decimal;
use tokio::sync::Mutex;

use rara_domain::contract::Contract;
use rara_domain::trading::git::{
    AddResult, CommitLogEntry, CommitPrepareResult, GitExportState, GitPosition, GitState,
    GitStatus, OrderStatusUpdate, PushResult, RejectResult, SyncResult,
};
use rara_domain::trading::operation::{Operation, OperationOrderType, OperationResult, OperationStatus};
use rara_domain::trading::{OrderType, Side};

use crate::broker::{AccountInfo, Broker, BrokerError, OrderStatus, Position};
use crate::health::{BrokerHealth, BrokerHealthInfo, HealthTracker};
use crate::trading_git::{
    OperationDispatcher, PendingOrder, StateProvider, TradingGit, TradingGitError,
};

/// Unified Trading Account — the main entry point for the trading-as-git workflow.
///
/// Owns a [`Broker`], a [`TradingGit`] history tracker, and a [`HealthTracker`].
/// All broker calls are routed through health tracking, and all operations
/// flow through the git stage -> commit -> push pipeline.
pub struct UnifiedTradingAccount {
    /// Account identifier.
    pub id: String,
    /// Human-readable label.
    pub label: String,
    broker: Box<dyn Broker>,
    git: Mutex<TradingGit>,
    health: Mutex<HealthTracker>,
}

impl UnifiedTradingAccount {
    /// Create a new UTA with an empty git history.
    pub fn new(id: impl Into<String>, label: impl Into<String>, broker: Box<dyn Broker>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            broker,
            git: Mutex::new(TradingGit::new()),
            health: Mutex::new(HealthTracker::new()),
        }
    }

    /// Create a new UTA restoring git history from a saved state.
    pub fn with_saved_state(
        id: impl Into<String>,
        label: impl Into<String>,
        broker: Box<dyn Broker>,
        saved_state: GitExportState,
    ) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            broker,
            git: Mutex::new(TradingGit::restore(saved_state)),
            health: Mutex::new(HealthTracker::new()),
        }
    }

    // ── Stage operations ───────────────────────────────────────────────

    /// Stage a new order placement.
    pub async fn stage_place_order(
        &self,
        contract: Contract,
        side: Side,
        order_type: OperationOrderType,
        quantity: Decimal,
        limit_price: Option<Decimal>,
    ) -> AddResult {
        self.git.lock().await.add(Operation::PlaceOrder {
            contract,
            side,
            order_type,
            quantity,
            limit_price,
        })
    }

    /// Stage an order modification.
    pub async fn stage_modify_order(
        &self,
        order_id: impl Into<String>,
        quantity: Option<Decimal>,
        price: Option<Decimal>,
    ) -> AddResult {
        self.git.lock().await.add(Operation::ModifyOrder {
            order_id: order_id.into(),
            quantity,
            price,
        })
    }

    /// Stage a position close.
    pub async fn stage_close_position(
        &self,
        contract: Contract,
        quantity: Option<Decimal>,
    ) -> AddResult {
        self.git.lock().await.add(Operation::ClosePosition {
            contract,
            quantity,
        })
    }

    /// Stage an order cancellation.
    pub async fn stage_cancel_order(&self, order_id: impl Into<String>) -> AddResult {
        self.git.lock().await.add(Operation::CancelOrder {
            order_id: order_id.into(),
        })
    }

    // ── Git workflow ───────────────────────────────────────────────────

    /// Commit staged operations, preparing them for push.
    pub async fn commit(&self, message: &str) -> Option<CommitPrepareResult> {
        self.git.lock().await.commit(message)
    }

    /// Push the pending commit — dispatches operations to the broker.
    ///
    /// Uses a split-lock pattern: the git mutex is only held briefly to take
    /// pending data and record the commit, never during broker I/O.
    pub async fn push(&self) -> Result<PushResult, TradingGitError> {
        // Phase 1: take pending (short lock)
        let (hash, message, operations) = self.git.lock().await.take_pending()?;

        // Phase 2: dispatch without holding lock
        let dispatcher = BrokerDispatcher {
            broker: self.broker.as_ref(),
            health: &self.health,
        };
        let mut results = Vec::with_capacity(operations.len());
        for op in &operations {
            results.push(dispatcher.dispatch(op).await);
        }

        // Phase 3: snapshot state without holding lock
        let state_provider = BrokerStateProvider {
            broker: self.broker.as_ref(),
            health: &self.health,
        };
        let state_after = state_provider
            .get_state()
            .await
            .map_err(|source| TradingGitError::Broker { source })?;

        // Phase 4: record commit (short lock)
        let push_result =
            self.git
                .lock()
                .await
                .record_push(hash, message, operations, results, state_after);
        Ok(push_result)
    }

    /// Reject the pending commit without executing any operations.
    ///
    /// Uses a split-lock pattern: the git mutex is only held briefly to take
    /// pending data and record the rejection, never during broker I/O.
    pub async fn reject(&self, reason: &str) -> Result<RejectResult, TradingGitError> {
        // Phase 1: take pending (short lock)
        let (hash, message, operations) = self.git.lock().await.take_pending()?;

        // Phase 2: snapshot state without holding lock
        let state_provider = BrokerStateProvider {
            broker: self.broker.as_ref(),
            health: &self.health,
        };
        let state_after = state_provider
            .get_state()
            .await
            .map_err(|source| TradingGitError::Broker { source })?;

        // Phase 3: record rejection (short lock)
        let reject_result =
            self.git
                .lock()
                .await
                .record_reject(hash, &message, operations, reason, state_after);
        Ok(reject_result)
    }

    /// Sync order statuses from the broker and record as a sync commit.
    pub async fn sync(&self) -> Result<SyncResult, TradingGitError> {
        // Fetch current order statuses for all pending orders
        let pending = self.git.lock().await.pending_order_ids();

        if pending.is_empty() {
            let state_provider = BrokerStateProvider {
                broker: self.broker.as_ref(),
                health: &self.health,
            };
            return self
                .git
                .lock()
                .await
                .sync(Vec::new(), &state_provider)
                .await;
        }

        let order_ids: Vec<String> = pending.iter().map(|p| p.order_id.clone()).collect();
        let orders = call_broker_with_health(self.broker.as_ref(), &self.health, |b| {
            Box::pin(async move { b.get_orders(&order_ids).await })
        })
        .await
        .map_err(|source| TradingGitError::Broker { source })?;

        let updates: Vec<OrderStatusUpdate> = orders
            .iter()
            .filter_map(|o| {
                let symbol = pending
                    .iter()
                    .find(|p| p.order_id == o.order_id)
                    .map_or_else(|| "unknown".to_string(), |p| p.symbol.clone());

                let current_status = match o.status {
                    OrderStatus::Filled => OperationStatus::Filled,
                    OrderStatus::Cancelled => OperationStatus::Cancelled,
                    OrderStatus::Rejected => OperationStatus::Rejected,
                    OrderStatus::Submitted => return None, // no change
                };

                Some(OrderStatusUpdate {
                    order_id: o.order_id.clone(),
                    symbol,
                    previous_status: OperationStatus::Submitted,
                    current_status,
                    filled_price: o.avg_fill_price,
                    filled_qty: Some(o.quantity),
                })
            })
            .collect();

        let state_provider = BrokerStateProvider {
            broker: self.broker.as_ref(),
            health: &self.health,
        };
        self.git.lock().await.sync(updates, &state_provider).await
    }

    // ── Queries ────────────────────────────────────────────────────────

    /// Return the current staging area and commit state.
    pub async fn status(&self) -> GitStatus {
        self.git.lock().await.status()
    }

    /// Return commit log entries, newest first.
    pub async fn log(&self, limit: usize, symbol: Option<&str>) -> Vec<CommitLogEntry> {
        self.git.lock().await.log(limit, symbol)
    }

    /// Look up a commit by hash. Returns a cloned commit if found.
    pub async fn show(&self, hash: &str) -> Option<rara_domain::trading::git::GitCommit> {
        self.git.lock().await.show(hash).cloned()
    }

    /// Return all orders still in pending (submitted) state.
    pub async fn pending_order_ids(&self) -> Vec<PendingOrder> {
        self.git.lock().await.pending_order_ids()
    }

    /// Export the full git state for persistence.
    pub async fn export_git_state(&self) -> GitExportState {
        self.git.lock().await.export_state()
    }

    // ── Broker delegation (with health tracking) ───────────────────────

    /// Fetch account information from the broker.
    pub async fn get_account(&self) -> Result<AccountInfo, BrokerError> {
        call_broker_with_health(self.broker.as_ref(), &self.health, |b| {
            Box::pin(async move { b.account_info().await })
        })
        .await
    }

    /// Fetch current positions from the broker.
    pub async fn get_positions(&self) -> Result<Vec<Position>, BrokerError> {
        call_broker_with_health(self.broker.as_ref(), &self.health, |b| {
            Box::pin(async move { b.positions().await })
        })
        .await
    }

    // ── Health ─────────────────────────────────────────────────────────

    /// Return the current health status.
    pub async fn health(&self) -> BrokerHealth {
        self.health.lock().await.status()
    }

    /// Return a detailed health info snapshot.
    pub async fn health_info(&self) -> BrokerHealthInfo {
        self.health.lock().await.info()
    }

    /// Check whether the broker has been manually disabled.
    pub async fn is_disabled(&self) -> bool {
        self.health.lock().await.is_disabled()
    }

    /// Re-enable a previously disabled broker.
    pub async fn enable(&self) {
        self.health.lock().await.enable();
    }

    /// Set the current trading round for subsequent commits.
    pub async fn set_current_round(&self, round: u32) {
        self.git.lock().await.set_current_round(round);
    }
}

// ── Internal adapter: Broker → OperationDispatcher ─────────────────────

/// Adapts a [`Broker`] reference to the [`OperationDispatcher`] trait.
struct BrokerDispatcher<'a> {
    broker: &'a dyn Broker,
    health: &'a Mutex<HealthTracker>,
}

#[async_trait]
impl OperationDispatcher for BrokerDispatcher<'_> {
    async fn dispatch(&self, op: &Operation) -> OperationResult {
        let result = match op {
            Operation::PlaceOrder {
                contract,
                side,
                order_type,
                quantity,
                limit_price,
            } => {
                let broker_order_type = map_order_type(*order_type);
                self.broker
                    .place_order(contract, *side, broker_order_type, *quantity, *limit_price)
                    .await
            }
            Operation::ModifyOrder {
                order_id,
                quantity,
                price,
            } => self.broker.modify_order(order_id, *quantity, *price).await,
            Operation::ClosePosition { contract, quantity } => {
                self.broker.close_position(contract, *quantity).await
            }
            Operation::CancelOrder { order_id } => self.broker.cancel_order(order_id).await,
            Operation::SyncOrders => {
                // Sync is handled at the UTA level, not via dispatch
                return OperationResult {
                    action: op.clone(),
                    success: true,
                    order_id: None,
                    status: OperationStatus::Filled,
                    filled_qty: None,
                    filled_price: None,
                    error: None,
                };
            }
        };

        match result {
            Ok(order_result) => {
                self.health.lock().await.record_success();
                OperationResult {
                    action: op.clone(),
                    success: order_result.status != OrderStatus::Rejected,
                    order_id: Some(order_result.order_id),
                    status: map_order_status(order_result.status),
                    filled_qty: None,
                    filled_price: None,
                    error: None,
                }
            }
            Err(e) => {
                let msg = e.to_string();
                self.health.lock().await.record_failure(&msg);
                OperationResult {
                    action: op.clone(),
                    success: false,
                    order_id: None,
                    status: OperationStatus::Rejected,
                    filled_qty: None,
                    filled_price: None,
                    error: Some(msg),
                }
            }
        }
    }
}

// ── Internal adapter: Broker → StateProvider ───────────────────────────

/// Adapts a [`Broker`] reference to the [`StateProvider`] trait.
struct BrokerStateProvider<'a> {
    broker: &'a dyn Broker,
    health: &'a Mutex<HealthTracker>,
}

#[async_trait]
impl StateProvider for BrokerStateProvider<'_> {
    async fn get_state(&self) -> Result<GitState, BrokerError> {
        let account = call_broker_with_health(self.broker, self.health, |b| {
            Box::pin(async move { b.account_info().await })
        })
        .await?;

        let positions = account
            .positions
            .iter()
            .map(|p| GitPosition {
                contract_id: p.contract_id.clone(),
                side: p.side,
                quantity: p.quantity,
                avg_cost: p.avg_entry_price,
                market_price: p.avg_entry_price, // paper broker doesn't track market price separately
                unrealized_pnl: p.unrealized_pnl,
            })
            .collect();

        let total_unrealized: Decimal = account.positions.iter().map(|p| p.unrealized_pnl).sum();

        Ok(GitState {
            net_liquidation: account.total_equity,
            total_cash_value: account.available_cash,
            unrealized_pnl: total_unrealized,
            // TODO: AccountInfo does not carry realized_pnl — always zero until Broker trait is extended
            realized_pnl: Decimal::ZERO,
            positions,
        })
    }
}

// ── Helpers ────────────────────────────────────────────────────────────

/// Map [`OperationOrderType`] to the broker's [`OrderType`].
const fn map_order_type(op_type: OperationOrderType) -> OrderType {
    match op_type {
        OperationOrderType::Market => OrderType::Market,
        OperationOrderType::Limit => OrderType::Limit,
        OperationOrderType::Stop => OrderType::StopLoss,
        OperationOrderType::StopLimit => OrderType::StopLimit,
    }
}

/// Map broker [`OrderStatus`] to the operation's [`OperationStatus`].
const fn map_order_status(status: OrderStatus) -> OperationStatus {
    match status {
        OrderStatus::Submitted => OperationStatus::Submitted,
        OrderStatus::Filled => OperationStatus::Filled,
        OrderStatus::Rejected => OperationStatus::Rejected,
        OrderStatus::Cancelled => OperationStatus::Cancelled,
    }
}

/// Call a broker method and update health tracking based on the result.
async fn call_broker_with_health<'a, T, F>(
    broker: &'a dyn Broker,
    health: &Mutex<HealthTracker>,
    f: F,
) -> Result<T, BrokerError>
where
    F: FnOnce(&'a dyn Broker) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<T, BrokerError>> + Send + 'a>>,
{
    let result = f(broker).await;
    match &result {
        Ok(_) => health.lock().await.record_success(),
        Err(e) => health.lock().await.record_failure(&e.to_string()),
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    use crate::brokers::paper::PaperBroker;
    use rara_domain::contract::{Contract, SecType};
    use rara_domain::trading::operation::OperationOrderType;

    fn test_contract() -> Contract {
        Contract::builder()
            .symbol("BTCUSDT")
            .exchange("binance")
            .sec_type(SecType::CryptoSpot)
            .currency("USDT")
            .build()
    }

    fn test_uta() -> UnifiedTradingAccount {
        UnifiedTradingAccount::new(
            "test-uta",
            "Test UTA",
            Box::new(PaperBroker::new(dec!(50000))),
        )
    }

    #[tokio::test]
    async fn stage_commit_push_lifecycle() {
        let uta = test_uta();

        // Stage an order
        let add = uta
            .stage_place_order(
                test_contract(),
                Side::Buy,
                OperationOrderType::Market,
                dec!(0.1),
                None,
            )
            .await;
        assert!(add.staged);

        // Commit
        let commit = uta.commit("buy BTC").await.expect("should commit");
        assert!(commit.prepared);
        assert_eq!(commit.operation_count, 1);

        // Push — executes against PaperBroker
        let push = uta.push().await.expect("push should succeed");
        assert_eq!(push.operation_count, 1);
        assert_eq!(push.submitted.len(), 1);
        assert!(push.rejected.is_empty());

        // Verify commit appears in log
        let log = uta.log(10, None).await;
        assert_eq!(log.len(), 1);
        assert!(log[0].message.contains("buy BTC"));

        // Verify commit retrievable by hash
        let shown = uta.show(&push.hash).await.expect("commit should exist");
        assert_eq!(shown.operations.len(), 1);
    }

    #[tokio::test]
    async fn reject_discards_without_execution() {
        let uta = test_uta();

        uta.stage_place_order(
            test_contract(),
            Side::Buy,
            OperationOrderType::Market,
            dec!(1),
            None,
        )
        .await;
        uta.commit("risky trade").await.expect("should commit");

        // Reject instead of pushing
        let reject = uta.reject("too risky").await.expect("reject should succeed");
        assert_eq!(reject.operation_count, 1);
        assert!(reject.message.contains("REJECTED"));

        // Verify no positions were opened on the broker
        let positions = uta.get_positions().await.expect("should get positions");
        assert!(positions.is_empty(), "no orders should have been executed");

        // Verify the rejected commit is in the log with UserRejected status
        let log = uta.log(10, None).await;
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].operations[0].status, OperationStatus::UserRejected);
    }

    #[tokio::test]
    async fn health_starts_healthy() {
        let uta = test_uta();
        assert_eq!(uta.health().await, BrokerHealth::Healthy);
        assert!(!uta.is_disabled().await);
    }
}
