//! Rich operation types for the UTA trading-as-git workflow.

use bon::Builder;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use strum::{Display, EnumString};

use super::Side;
use crate::contract::Contract;

/// A single trading operation in the UTA stage->commit->push workflow.
///
/// Each variant carries only the data relevant to that action,
/// mirroring `OpenAlice`'s discriminated-union `Operation` type.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum Operation {
    /// Place a new order on the exchange.
    PlaceOrder {
        /// Target contract.
        contract: Contract,
        /// Buy or sell.
        side: Side,
        /// Order type (market, limit, etc.).
        order_type: OperationOrderType,
        /// Order quantity.
        quantity: Decimal,
        /// Limit/trigger price (required for limit/stop orders).
        limit_price: Option<Decimal>,
    },
    /// Modify an existing open order.
    ModifyOrder {
        /// Broker-assigned order ID to modify.
        order_id: String,
        /// New quantity (if changing).
        quantity: Option<Decimal>,
        /// New price (if changing).
        price: Option<Decimal>,
    },
    /// Close an existing position.
    ClosePosition {
        /// Contract to close.
        contract: Contract,
        /// Quantity to close; `None` means close entire position.
        quantity: Option<Decimal>,
    },
    /// Cancel a pending order.
    CancelOrder {
        /// Broker-assigned order ID to cancel.
        order_id: String,
    },
    /// Sync order statuses from the broker.
    SyncOrders,
}

impl Operation {
    /// Extract the symbol string from this operation, if available.
    pub fn symbol(&self) -> Option<&str> {
        match self {
            Self::PlaceOrder { contract, .. } | Self::ClosePosition { contract, .. } => {
                Some(&contract.symbol)
            }
            _ => None,
        }
    }
}

/// Order type used within [`Operation`].
///
/// Separate from `rara_domain::trading::OrderType` to allow future UTA-specific
/// extensions (e.g. trailing stop) without polluting the core enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Display, EnumString)]
pub enum OperationOrderType {
    /// Execute at current market price.
    Market,
    /// Execute at a specific price or better.
    Limit,
    /// Trigger a market order at a stop price.
    Stop,
    /// Trigger a limit order at a stop price.
    StopLimit,
}

/// Lifecycle status of a dispatched operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Display, EnumString)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum OperationStatus {
    /// Order accepted and pending execution.
    Submitted,
    /// Order fully filled.
    Filled,
    /// Order rejected by broker or guard.
    Rejected,
    /// Order cancelled.
    Cancelled,
    /// User discarded the commit before execution.
    UserRejected,
}

/// Result of dispatching a single operation to the broker.
#[derive(Debug, Clone, Serialize, Deserialize, Builder)]
pub struct OperationResult {
    /// Which action was attempted.
    pub action: Operation,
    /// Whether the broker accepted the operation.
    pub success: bool,
    /// Broker-assigned order ID (if any).
    pub order_id: Option<String>,
    /// Lifecycle status after dispatch.
    pub status: OperationStatus,
    /// Filled quantity (if partially/fully filled).
    pub filled_qty: Option<Decimal>,
    /// Average fill price.
    pub filled_price: Option<Decimal>,
    /// Error message on failure.
    pub error: Option<String>,
}
