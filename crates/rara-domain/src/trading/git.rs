//! Pure domain types for the `TradingGit` commit history.

use bon::Builder;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use super::{
    Side,
    operation::{Operation, OperationResult, OperationStatus},
};

/// 8-character hex commit hash.
pub type CommitHash = String;

/// Account-level state snapshot captured after each commit.
#[derive(Debug, Clone, Serialize, Deserialize, Builder)]
pub struct GitState {
    /// Total portfolio equity.
    pub net_liquidation:  Decimal,
    /// Available cash.
    pub total_cash_value: Decimal,
    /// Unrealized P&L across all positions.
    pub unrealized_pnl:   Decimal,
    /// Realized P&L.
    pub realized_pnl:     Decimal,
    /// Open positions at snapshot time.
    pub positions:        Vec<GitPosition>,
}

/// Position representation within a [`GitState`] snapshot.
#[derive(Debug, Clone, Serialize, Deserialize, Builder)]
pub struct GitPosition {
    /// Contract identifier.
    #[builder(into)]
    pub contract_id:    String,
    /// Long or short.
    pub side:           Side,
    /// Position size.
    pub quantity:       Decimal,
    /// Average entry price.
    pub avg_cost:       Decimal,
    /// Current market price. May be approximate if the broker does not provide
    /// real-time quotes.
    pub market_price:   Decimal,
    /// Unrealized P&L for this position.
    pub unrealized_pnl: Decimal,
}

/// An immutable git-style commit recording operations and their results.
#[derive(Debug, Clone, Serialize, Deserialize, Builder)]
pub struct GitCommit {
    /// Commit hash (8 hex chars).
    #[builder(into)]
    pub hash:        CommitHash,
    /// Parent commit hash (`None` for the first commit).
    pub parent_hash: Option<CommitHash>,
    /// Human-readable commit message.
    #[builder(into)]
    pub message:     String,
    /// Operations that were staged.
    pub operations:  Vec<Operation>,
    /// Per-operation execution results.
    pub results:     Vec<OperationResult>,
    /// Account state snapshot taken after execution.
    pub state_after: GitState,
    /// ISO 8601 timestamp.
    #[builder(into)]
    pub timestamp:   String,
    /// Optional trading round number.
    pub round:       Option<u32>,
}

/// Result of staging an operation via `TradingGit::add`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AddResult {
    /// Always `true` on success.
    pub staged:    bool,
    /// Index in the staging area.
    pub index:     usize,
    /// The staged operation.
    pub operation: Operation,
}

/// Result of preparing a commit via `TradingGit::commit`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommitPrepareResult {
    /// Always `true` on success.
    pub prepared:        bool,
    /// Generated commit hash.
    pub hash:            CommitHash,
    /// Commit message.
    pub message:         String,
    /// Number of staged operations.
    pub operation_count: usize,
}

/// Result of executing a commit via `TradingGit::push`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PushResult {
    /// Commit hash.
    pub hash:            CommitHash,
    /// Commit message.
    pub message:         String,
    /// Number of operations.
    pub operation_count: usize,
    /// Operations that were successfully dispatched.
    pub submitted:       Vec<OperationResult>,
    /// Operations rejected by guards or broker.
    pub rejected:        Vec<OperationResult>,
}

/// Result of rejecting a pending commit.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RejectResult {
    /// Commit hash of the rejected commit.
    pub hash:            CommitHash,
    /// Commit message.
    pub message:         String,
    /// Number of operations that were discarded.
    pub operation_count: usize,
}

/// Current state of the `TradingGit` staging area.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitStatus {
    /// Currently staged operations.
    pub staged:          Vec<Operation>,
    /// Message of the pending (committed but not pushed) commit.
    pub pending_message: Option<String>,
    /// Hash of the pending commit.
    pub pending_hash:    Option<CommitHash>,
    /// HEAD commit hash.
    pub head:            Option<CommitHash>,
    /// Total number of commits in history.
    pub commit_count:    usize,
}

/// Summary of a single operation within a commit log entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperationSummary {
    /// Symbol involved.
    pub symbol: String,
    /// Action type description.
    pub action: String,
    /// Human-readable change description.
    pub change: String,
    /// Outcome status.
    pub status: OperationStatus,
}

/// Abbreviated commit entry for log display.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommitLogEntry {
    /// Commit hash.
    pub hash:        CommitHash,
    /// Parent commit hash.
    pub parent_hash: Option<CommitHash>,
    /// Commit message.
    pub message:     String,
    /// ISO 8601 timestamp.
    pub timestamp:   String,
    /// Trading round number.
    pub round:       Option<u32>,
    /// Operation summaries.
    pub operations:  Vec<OperationSummary>,
}

/// Serializable export of the full `TradingGit` state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitExportState {
    /// All commits in chronological order.
    pub commits: Vec<GitCommit>,
    /// Current HEAD hash.
    pub head:    Option<CommitHash>,
}

/// A single order status update from a sync operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderStatusUpdate {
    /// Broker-assigned order ID.
    pub order_id:        String,
    /// Trading symbol.
    pub symbol:          String,
    /// Status before the update.
    pub previous_status: OperationStatus,
    /// Status after the update.
    pub current_status:  OperationStatus,
    /// Fill price if newly filled.
    pub filled_price:    Option<Decimal>,
    /// Fill quantity if newly filled.
    pub filled_qty:      Option<Decimal>,
}

/// Result of a sync operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncResult {
    /// Commit hash of the sync commit.
    pub hash:          CommitHash,
    /// Number of orders whose status changed.
    pub updated_count: usize,
    /// Individual status changes.
    pub updates:       Vec<OrderStatusUpdate>,
}
