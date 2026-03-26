//! Broker trait and associated types for order execution.

use async_trait::async_trait;
use bon::Builder;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use snafu::Snafu;

use rara_domain::contract::Contract;
use rara_domain::trading::{OrderType, Side, StagedAction};

/// Result of submitting an order to a broker.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderResult {
    /// Broker-assigned order identifier.
    pub order_id: String,
    /// Contract the order targets.
    pub contract_id: String,
    /// Current status of the order.
    pub status: OrderStatus,
}

/// Lifecycle status of an order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrderStatus {
    /// Order accepted and pending execution.
    Submitted,
    /// Order rejected by the broker.
    Rejected,
    /// Order fully filled.
    Filled,
    /// Order cancelled.
    Cancelled,
}

/// Detailed report of an order execution.
#[derive(Debug, Clone, Serialize, Deserialize, Builder)]
pub struct ExecutionReport {
    /// Broker-assigned order identifier.
    #[builder(into)]
    pub order_id: String,
    /// Contract the order targets.
    #[builder(into)]
    pub contract_id: String,
    /// Trade direction.
    pub side: Side,
    /// Filled quantity.
    pub quantity: Decimal,
    /// Execution price.
    pub price: Decimal,
    /// Current order status.
    pub status: OrderStatus,
    /// When the order was filled.
    pub filled_at: jiff::Timestamp,
}

/// A currently held position.
#[derive(Debug, Clone, Serialize, Deserialize, Builder)]
pub struct Position {
    /// Contract identifier.
    #[builder(into)]
    pub contract_id: String,
    /// Position direction.
    pub side: Side,
    /// Position size.
    pub quantity: Decimal,
    /// Average entry price.
    pub avg_entry_price: Decimal,
    /// Unrealized profit/loss.
    pub unrealized_pnl: Decimal,
}

/// Account-level information.
#[derive(Debug, Clone, Serialize, Deserialize, Builder)]
pub struct AccountInfo {
    /// Total equity including unrealized P&L.
    pub total_equity: Decimal,
    /// Available cash for new orders.
    pub available_cash: Decimal,
    /// Currently held positions.
    pub positions: Vec<Position>,
}

/// A currently open (unfilled) order.
#[derive(Debug, Clone, Serialize, Deserialize, Builder)]
pub struct OpenOrder {
    /// Broker-assigned order identifier.
    #[builder(into)]
    pub order_id: String,
    /// Contract this order targets.
    #[builder(into)]
    pub contract_id: String,
    /// Trade direction.
    pub side: Side,
    /// Order type.
    pub order_type: OrderType,
    /// Total requested quantity.
    pub quantity: Decimal,
    /// Limit price (for limit orders).
    pub limit_price: Option<Decimal>,
    /// Current order status.
    pub status: OrderStatus,
    /// Average fill price so far.
    pub avg_fill_price: Option<Decimal>,
}

/// Errors that can occur during broker operations.
#[derive(Debug, Snafu)]
#[snafu(visibility(pub))]
pub enum BrokerError {
    /// Failed to connect to the broker.
    #[snafu(display("connection error: {message}"))]
    Connection {
        /// Description of the connection failure.
        message: String,
    },
    /// Broker rejected the order.
    #[snafu(display("order rejected: {message}"))]
    OrderRejected {
        /// Reason for rejection.
        message: String,
    },
    /// Authentication failed.
    #[snafu(display("authentication error: {message}"))]
    Authentication {
        /// Description of the authentication failure.
        message: String,
    },
    /// Rate limit exceeded.
    #[snafu(display("rate limit exceeded: {message}"))]
    RateLimit {
        /// Description of the rate limit error.
        message: String,
    },
    /// Exchange returned an unexpected response.
    #[snafu(display("exchange error: {message}"))]
    Exchange {
        /// Description of the exchange error.
        message: String,
    },
    /// Unsupported operation for the current action type.
    #[snafu(display("unsupported action: {message}"))]
    UnsupportedAction {
        /// Description of the unsupported action.
        message: String,
    },
}

/// Abstraction over a broker that can execute orders and report positions.
#[async_trait]
pub trait Broker: Send + Sync {
    /// Submit staged actions for execution.
    async fn push(&self, actions: &[StagedAction]) -> Result<Vec<OrderResult>, BrokerError>;

    /// Sync and return recent execution reports.
    async fn sync_orders(&self) -> Result<Vec<ExecutionReport>, BrokerError>;

    /// Return all currently held positions.
    async fn positions(&self) -> Result<Vec<Position>, BrokerError>;

    /// Return account-level information.
    async fn account_info(&self) -> Result<AccountInfo, BrokerError>;

    /// Place a single order. Default: delegates to `push` with one action.
    async fn place_order(
        &self,
        contract: &Contract,
        side: Side,
        order_type: OrderType,
        quantity: Decimal,
        limit_price: Option<Decimal>,
    ) -> Result<OrderResult, BrokerError> {
        let action = StagedAction::builder()
            .action_type(rara_domain::trading::ActionType::PlaceOrder)
            .contract_id(contract.id())
            .side(side)
            .quantity(quantity)
            .order_type(order_type)
            .maybe_limit_price(limit_price)
            .build();
        let mut results = self.push(&[action]).await?;
        results.pop().ok_or_else(|| BrokerError::Exchange {
            message: "broker returned no results".into(),
        })
    }

    /// Cancel an open order.
    async fn cancel_order(&self, _order_id: &str) -> Result<OrderResult, BrokerError> {
        Err(BrokerError::UnsupportedAction {
            message: "cancel_order not supported by this broker".into(),
        })
    }

    /// Modify an existing order.
    async fn modify_order(
        &self,
        _order_id: &str,
        _quantity: Option<Decimal>,
        _price: Option<Decimal>,
    ) -> Result<OrderResult, BrokerError> {
        Err(BrokerError::UnsupportedAction {
            message: "modify_order not supported by this broker".into(),
        })
    }

    /// Close a position.
    async fn close_position(
        &self,
        contract: &Contract,
        _quantity: Option<Decimal>,
    ) -> Result<OrderResult, BrokerError> {
        Err(BrokerError::UnsupportedAction {
            message: format!(
                "close_position not supported by this broker for {}",
                contract.id()
            ),
        })
    }

    /// Query status of specific orders by ID.
    async fn get_orders(&self, _order_ids: &[String]) -> Result<Vec<OpenOrder>, BrokerError> {
        Err(BrokerError::UnsupportedAction {
            message: "get_orders not supported by this broker".into(),
        })
    }
}
