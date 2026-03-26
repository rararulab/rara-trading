//! Git-like operation tracker for the UTA (Unified Trading Account) workflow.
//!
//! Provides a stage -> commit -> push workflow mirroring git semantics:
//! - `add` stages operations
//! - `commit` prepares a batch with a hash and message
//! - `push` dispatches operations to the broker and records results

use async_trait::async_trait;
use sha2::{Digest, Sha256};
use snafu::{OptionExt, ResultExt, Snafu};

use crate::broker::BrokerError;
use rara_domain::trading::git::{
    AddResult, CommitHash, CommitLogEntry, CommitPrepareResult, GitCommit, GitExportState,
    GitState, GitStatus, OperationSummary, OrderStatusUpdate, PushResult, RejectResult, SyncResult,
};
use rara_domain::trading::operation::{Operation, OperationResult, OperationStatus};

/// Errors from `TradingGit` operations.
#[derive(Debug, Snafu)]
#[snafu(visibility(pub))]
pub enum TradingGitError {
    /// Push or reject called without a prior commit.
    #[snafu(display("no pending commit — call commit() before push/reject"))]
    NoPendingCommit,
    /// Broker error during state snapshot.
    #[snafu(display("broker error: {source}"))]
    Broker { source: BrokerError },
}

/// Convenience alias for `TradingGit` results.
pub type Result<T> = std::result::Result<T, TradingGitError>;

/// Dispatches a single [`Operation`] to the broker and returns the result.
#[async_trait]
pub trait OperationDispatcher: Send + Sync {
    /// Execute one operation and return the broker's response.
    async fn dispatch(&self, op: &Operation) -> OperationResult;
}

/// Provides a snapshot of the current account/position state.
#[async_trait]
pub trait StateProvider: Send + Sync {
    /// Capture the current account state for recording in a commit.
    async fn get_state(&self) -> std::result::Result<GitState, BrokerError>;
}

/// An order still pending execution (submitted but not yet filled/cancelled).
#[derive(Debug, Clone)]
pub struct PendingOrder {
    /// Broker-assigned order ID.
    pub order_id: String,
    /// Trading symbol.
    pub symbol: String,
}

/// Core git-like operation tracker for the UTA workflow.
///
/// All state is held in memory. Use [`export_state`](TradingGit::export_state) /
/// [`restore`](TradingGit::restore) for persistence.
pub struct TradingGit {
    /// Operations staged but not yet committed.
    staging: Vec<Operation>,
    /// Message of the committed-but-not-pushed batch.
    pending_message: Option<String>,
    /// Hash of the committed-but-not-pushed batch.
    pending_hash: Option<CommitHash>,
    /// Immutable commit history.
    commits: Vec<GitCommit>,
    /// Current HEAD commit hash.
    head: Option<CommitHash>,
    /// Optional trading round number attached to new commits.
    current_round: Option<u32>,
}

impl TradingGit {
    /// Create a new, empty `TradingGit` instance.
    pub const fn new() -> Self {
        Self {
            staging: Vec::new(),
            pending_message: None,
            pending_hash: None,
            commits: Vec::new(),
            head: None,
            current_round: None,
        }
    }

    /// Restore from a previously exported state.
    pub fn restore(state: GitExportState) -> Self {
        let head = state.head.clone();
        Self {
            staging: Vec::new(),
            pending_message: None,
            pending_hash: None,
            commits: state.commits,
            head,
            current_round: None,
        }
    }

    /// Set the current trading round number for subsequent commits.
    pub const fn set_current_round(&mut self, round: u32) {
        self.current_round = Some(round);
    }

    /// Stage an operation for the next commit.
    pub fn add(&mut self, operation: Operation) -> AddResult {
        self.staging.push(operation.clone());
        AddResult {
            staged: true,
            index: self.staging.len() - 1,
            operation,
        }
    }

    /// Prepare a commit from staged operations.
    ///
    /// Generates a hash and moves staged ops into a pending state.
    /// Returns `None` if no operations are staged.
    pub fn commit(&mut self, message: &str) -> Option<CommitPrepareResult> {
        if self.staging.is_empty() {
            return None;
        }

        let hash = generate_commit_hash(message, &self.staging);
        self.pending_message = Some(message.to_string());
        self.pending_hash = Some(hash.clone());

        Some(CommitPrepareResult {
            prepared: true,
            hash,
            message: message.to_string(),
            operation_count: self.staging.len(),
        })
    }

    /// Execute the pending commit by dispatching all staged operations.
    ///
    /// Each operation is sent to the `dispatcher`; after execution a state
    /// snapshot is taken via `state_provider` and the commit is recorded.
    pub async fn push(
        &mut self,
        dispatcher: &dyn OperationDispatcher,
        state_provider: &dyn StateProvider,
    ) -> Result<PushResult> {
        let (hash, message, operations) = self.take_pending()?;

        let mut results = Vec::with_capacity(operations.len());
        for op in &operations {
            results.push(dispatcher.dispatch(op).await);
        }

        let state_after = state_provider.get_state().await.context(BrokerSnafu)?;

        Ok(self.record_push(hash, message, operations, results, state_after))
    }

    /// Reject the pending commit without executing any operations.
    ///
    /// All staged operations are marked as [`OperationStatus::UserRejected`]
    /// and a commit is recorded for auditability.
    pub async fn reject(
        &mut self,
        reason: &str,
        state_provider: &dyn StateProvider,
    ) -> Result<RejectResult> {
        let (hash, message, operations) = self.take_pending()?;
        let state_after = state_provider.get_state().await.context(BrokerSnafu)?;

        Ok(self.record_reject(hash, &message, operations, reason, state_after))
    }

    /// Record order status updates as a sync commit.
    ///
    /// Creates a `SyncOrders` commit capturing which orders changed status.
    pub async fn sync(
        &mut self,
        updates: Vec<OrderStatusUpdate>,
        state_provider: &dyn StateProvider,
    ) -> Result<SyncResult> {
        let state_after = state_provider.get_state().await.context(BrokerSnafu)?;
        Ok(self.record_sync(updates, state_after))
    }

    /// Record a sync commit with pre-fetched state (split-lock friendly).
    pub fn record_sync(
        &mut self,
        updates: Vec<OrderStatusUpdate>,
        state_after: GitState,
    ) -> SyncResult {
        let updated_count = updates.len();
        let message = format!("sync: {updated_count} order status update(s)");
        let hash = generate_commit_hash(&message, &updates);

        let results = vec![OperationResult {
            action: Operation::SyncOrders,
            success: true,
            order_id: None,
            status: OperationStatus::Submitted,
            filled_qty: None,
            filled_price: None,
            error: None,
        }];

        let commit = GitCommit {
            hash: hash.clone(),
            parent_hash: self.head.clone(),
            message,
            operations: vec![Operation::SyncOrders],
            results,
            state_after,
            timestamp: jiff::Timestamp::now().to_string(),
            round: self.current_round,
        };

        self.head = Some(hash.clone());
        self.commits.push(commit);

        SyncResult {
            hash,
            updated_count,
            updates,
        }
    }

    /// Take the pending commit data (hash, message, operations) from staging.
    ///
    /// Returns an error if no pending commit exists.
    pub fn take_pending(&mut self) -> Result<(CommitHash, String, Vec<Operation>)> {
        let hash = self.pending_hash.take().context(NoPendingCommitSnafu)?;
        let message = self.pending_message.take().context(NoPendingCommitSnafu)?;
        let operations = self.staging.drain(..).collect();
        Ok((hash, message, operations))
    }

    /// Record a completed push as an immutable commit.
    pub fn record_push(
        &mut self,
        hash: CommitHash,
        message: String,
        operations: Vec<Operation>,
        results: Vec<OperationResult>,
        state_after: GitState,
    ) -> PushResult {
        let submitted: Vec<_> = results.iter().filter(|r| r.success).cloned().collect();
        let rejected: Vec<_> = results.iter().filter(|r| !r.success).cloned().collect();
        let operation_count = operations.len();

        let commit = GitCommit {
            hash: hash.clone(),
            parent_hash: self.head.clone(),
            message: message.clone(),
            operations,
            results,
            state_after,
            timestamp: jiff::Timestamp::now().to_string(),
            round: self.current_round,
        };

        self.head = Some(hash.clone());
        self.commits.push(commit);

        PushResult {
            hash,
            message,
            operation_count,
            submitted,
            rejected,
        }
    }

    /// Record a rejected commit (all operations marked `UserRejected`).
    pub fn record_reject(
        &mut self,
        hash: CommitHash,
        message: &str,
        operations: Vec<Operation>,
        reason: &str,
        state_after: GitState,
    ) -> RejectResult {
        let operation_count = operations.len();
        let results: Vec<OperationResult> = operations
            .iter()
            .map(|op| OperationResult {
                action: op.clone(),
                success: false,
                order_id: None,
                status: OperationStatus::UserRejected,
                filled_qty: None,
                filled_price: None,
                error: Some(reason.to_string()),
            })
            .collect();

        let reject_message = format!("REJECTED: {message} — {reason}");
        let commit = GitCommit {
            hash: hash.clone(),
            parent_hash: self.head.clone(),
            message: reject_message.clone(),
            operations,
            results,
            state_after,
            timestamp: jiff::Timestamp::now().to_string(),
            round: self.current_round,
        };

        self.head = Some(hash.clone());
        self.commits.push(commit);

        RejectResult {
            hash,
            message: reject_message,
            operation_count,
        }
    }

    /// Return the current staging area and commit state.
    pub fn status(&self) -> GitStatus {
        GitStatus {
            staged: self.staging.clone(),
            pending_message: self.pending_message.clone(),
            pending_hash: self.pending_hash.clone(),
            head: self.head.clone(),
            commit_count: self.commits.len(),
        }
    }

    /// Return commit log entries, newest first, optionally filtered by symbol.
    pub fn log(&self, limit: usize, symbol: Option<&str>) -> Vec<CommitLogEntry> {
        self.commits
            .iter()
            .rev()
            .filter(|c| {
                symbol.is_none_or(|s| {
                    c.operations.iter().any(|op| op.symbol() == Some(s))
                })
            })
            .take(limit)
            .map(|c| CommitLogEntry {
                hash: c.hash.clone(),
                parent_hash: c.parent_hash.clone(),
                message: c.message.clone(),
                timestamp: c.timestamp.clone(),
                round: c.round,
                operations: build_operation_summaries(c),
            })
            .collect()
    }

    /// Look up a commit by its hash.
    pub fn show(&self, hash: &str) -> Option<&GitCommit> {
        self.commits.iter().find(|c| c.hash == hash)
    }

    /// Scan commit history for orders still in `Submitted` status.
    ///
    /// An order is considered pending if its most recent result across all
    /// commits is `Submitted` (i.e. never transitioned to Filled/Cancelled/Rejected).
    pub fn pending_order_ids(&self) -> Vec<PendingOrder> {
        use std::collections::HashMap;

        // Track latest status per order_id
        let mut latest: HashMap<String, (OperationStatus, String)> = HashMap::new();

        for commit in &self.commits {
            for result in &commit.results {
                if let Some(oid) = &result.order_id {
                    let symbol = result
                        .action
                        .symbol()
                        .unwrap_or("unknown")
                        .to_string();
                    latest.insert(oid.clone(), (result.status, symbol));
                }
            }
        }

        latest
            .into_iter()
            .filter(|(_, (status, _))| *status == OperationStatus::Submitted)
            .map(|(order_id, (_, symbol))| PendingOrder { order_id, symbol })
            .collect()
    }

    /// Export the full state for persistence.
    pub fn export_state(&self) -> GitExportState {
        GitExportState {
            commits: self.commits.clone(),
            head: self.head.clone(),
        }
    }
}

impl Default for TradingGit {
    fn default() -> Self {
        Self::new()
    }
}

/// Generate an 8-character hex commit hash from message + serialized content + timestamp.
fn generate_commit_hash(message: &str, content: &impl serde::Serialize) -> CommitHash {
    let mut hasher = Sha256::new();
    hasher.update(message.as_bytes());
    let json = serde_json::to_vec(content).expect("commit content must be serializable");
    hasher.update(&json);
    // Include timestamp for uniqueness even with identical content
    hasher.update(jiff::Timestamp::now().to_string().as_bytes());
    let result = hasher.finalize();
    hex::encode(&result[..4])
}

/// Build human-readable operation summaries for a commit.
fn build_operation_summaries(commit: &GitCommit) -> Vec<OperationSummary> {
    commit
        .operations
        .iter()
        .zip(commit.results.iter())
        .map(|(op, result)| {
            let (symbol, action, change) = match op {
                Operation::PlaceOrder {
                    contract,
                    side,
                    quantity,
                    order_type,
                    limit_price,
                } => {
                    let price_info = limit_price
                        .map(|p| format!(" @ {p}"))
                        .unwrap_or_default();
                    (
                        contract.symbol.clone(),
                        order_type.to_string(),
                        format!("{side} {quantity}{price_info}"),
                    )
                }
                Operation::ModifyOrder {
                    order_id,
                    quantity,
                    price,
                } => {
                    let changes: Vec<String> = [
                        quantity.map(|q| format!("qty={q}")),
                        price.map(|p| format!("price={p}")),
                    ]
                    .into_iter()
                    .flatten()
                    .collect();
                    (
                        "N/A".to_string(),
                        "ModifyOrder".to_string(),
                        format!("{order_id}: {}", changes.join(", ")),
                    )
                }
                Operation::ClosePosition { contract, quantity } => {
                    let qty_str = quantity
                        .map(|q| format!("{q}"))
                        .unwrap_or_else(|| "all".to_string());
                    (
                        contract.symbol.clone(),
                        "ClosePosition".to_string(),
                        format!("close {qty_str}"),
                    )
                }
                Operation::CancelOrder { order_id } => (
                    "N/A".to_string(),
                    "CancelOrder".to_string(),
                    format!("cancel {order_id}"),
                ),
                Operation::SyncOrders => (
                    "N/A".to_string(),
                    "SyncOrders".to_string(),
                    "sync order statuses".to_string(),
                ),
            };

            OperationSummary {
                symbol,
                action,
                change,
                status: result.status,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    use rara_domain::contract::Contract;
    use rara_domain::trading::operation::OperationOrderType;
    use rara_domain::trading::Side;

    /// Always-successful dispatcher for testing.
    struct SuccessDispatcher;

    #[async_trait]
    impl OperationDispatcher for SuccessDispatcher {
        async fn dispatch(&self, op: &Operation) -> OperationResult {
            OperationResult {
                action: op.clone(),
                success: true,
                order_id: Some("ORD-001".to_string()),
                status: OperationStatus::Submitted,
                filled_qty: None,
                filled_price: None,
                error: None,
            }
        }
    }

    /// Returns a fixed state snapshot.
    struct FixedState(GitState);

    #[async_trait]
    impl StateProvider for FixedState {
        async fn get_state(&self) -> std::result::Result<GitState, BrokerError> {
            Ok(self.0.clone())
        }
    }

    fn test_state() -> FixedState {
        FixedState(GitState {
            net_liquidation: dec!(10000),
            total_cash_value: dec!(5000),
            unrealized_pnl: dec!(100),
            realized_pnl: dec!(200),
            positions: vec![],
        })
    }

    fn test_place_order() -> Operation {
        Operation::PlaceOrder {
            contract: Contract::builder()
                .symbol("BTCUSDT")
                .exchange("binance")
                .sec_type(rara_domain::contract::SecType::CryptoSpot)
                .currency("USDT")
                .build(),
            side: Side::Buy,
            order_type: OperationOrderType::Market,
            quantity: dec!(0.1),
            limit_price: None,
        }
    }

    #[test]
    fn stage_commit_status_roundtrip() {
        let mut git = TradingGit::new();

        // Initially empty
        let status = git.status();
        assert!(status.staged.is_empty());
        assert!(status.pending_hash.is_none());
        assert_eq!(status.commit_count, 0);

        // Stage an operation
        let result = git.add(test_place_order());
        assert!(result.staged);
        assert_eq!(result.index, 0);

        let status = git.status();
        assert_eq!(status.staged.len(), 1);
        assert!(status.pending_hash.is_none(), "not committed yet");

        // Commit
        let commit_result = git.commit("buy BTC").expect("should commit");
        assert!(commit_result.prepared);
        assert_eq!(commit_result.operation_count, 1);
        assert_eq!(commit_result.hash.len(), 8, "hash should be 8 hex chars");

        let status = git.status();
        assert!(status.pending_hash.is_some(), "committed but not pushed");
        // Staging still holds ops until push drains them
        assert_eq!(status.staged.len(), 1);
    }

    #[test]
    fn commit_with_empty_staging_returns_none() {
        let mut git = TradingGit::new();
        assert!(git.commit("empty").is_none());
    }

    #[tokio::test]
    async fn push_executes_and_records_commit() {
        let mut git = TradingGit::new();
        git.set_current_round(1);
        git.add(test_place_order());
        git.commit("open position");

        let result = git
            .push(&SuccessDispatcher, &test_state())
            .await
            .expect("push should succeed");

        assert_eq!(result.operation_count, 1);
        assert_eq!(result.submitted.len(), 1);
        assert!(result.rejected.is_empty());

        // Commit is now in history
        let status = git.status();
        assert_eq!(status.commit_count, 1);
        assert!(status.pending_hash.is_none(), "pending cleared after push");
        assert!(status.staged.is_empty(), "staging cleared after push");
        assert_eq!(status.head, Some(result.hash.clone()));

        // Verify via show
        let commit = git.show(&result.hash).expect("commit should exist");
        assert_eq!(commit.round, Some(1));
        assert_eq!(commit.operations.len(), 1);
    }

    #[tokio::test]
    async fn reject_records_user_rejected_commit() {
        let mut git = TradingGit::new();
        git.add(test_place_order());
        git.commit("risky trade");

        let result = git
            .reject("too risky", &test_state())
            .await
            .expect("reject should succeed");

        assert_eq!(result.operation_count, 1);
        assert!(result.message.contains("REJECTED"));

        // Check the log shows UserRejected status
        let log = git.log(10, None);
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].operations[0].status, OperationStatus::UserRejected);
    }

    #[tokio::test]
    async fn export_and_restore_preserves_state() {
        let mut git = TradingGit::new();
        git.add(test_place_order());
        git.commit("trade 1");
        git.push(&SuccessDispatcher, &test_state())
            .await
            .expect("push should succeed");

        let exported = git.export_state();
        let original_head = exported.head.clone();
        let original_count = exported.commits.len();

        let restored = TradingGit::restore(exported);
        let restored_export = restored.export_state();

        assert_eq!(restored_export.head, original_head);
        assert_eq!(restored_export.commits.len(), original_count);
        assert_eq!(restored.status().commit_count, 1);
    }

    #[test]
    fn show_returns_none_for_unknown_hash() {
        let git = TradingGit::new();
        assert!(git.show("deadbeef").is_none());
    }

    #[tokio::test]
    async fn log_respects_limit() {
        let mut git = TradingGit::new();

        // Create 5 commits
        for i in 0..5 {
            git.add(test_place_order());
            git.commit(&format!("trade {i}"));
            git.push(&SuccessDispatcher, &test_state())
                .await
                .expect("push should succeed");
        }

        assert_eq!(git.status().commit_count, 5);

        // Limit to 3
        let log = git.log(3, None);
        assert_eq!(log.len(), 3);

        // Newest first: last commit message should be "trade 4"
        assert!(log[0].message.contains("trade 4"));
        assert!(log[2].message.contains("trade 2"));
    }
}
