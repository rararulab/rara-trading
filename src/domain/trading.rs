//! Trading action and commitment types.

use bon::Builder;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Direction of a trade.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Side {
    /// Buy / go long.
    Buy,
    /// Sell / go short.
    Sell,
}

/// Type of order to place.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum OrderType {
    /// Execute at current market price.
    Market,
    /// Execute at a specific price or better.
    Limit,
    /// Trigger a market order at a stop price.
    StopLoss,
    /// Trigger a limit order at a stop price.
    StopLimit,
}

/// Type of trading action to perform.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ActionType {
    /// Place a new order.
    PlaceOrder,
    /// Close an existing position.
    ClosePosition,
    /// Cancel a pending order.
    CancelOrder,
    /// Modify an existing order.
    ModifyOrder,
}

/// A single trading action staged for execution.
#[derive(Debug, Clone, Serialize, Deserialize, Builder)]
#[allow(clippy::struct_field_names)]
pub struct StagedAction {
    action_type: ActionType,
    #[builder(into)]
    contract_id: String,
    side: Side,
    quantity: Decimal,
    order_type: OrderType,
    limit_price: Option<Decimal>,
}

impl StagedAction {
    /// Returns the action type.
    pub const fn action_type(&self) -> ActionType {
        self.action_type
    }

    /// Returns the contract identifier.
    pub fn contract_id(&self) -> &str {
        &self.contract_id
    }

    /// Returns the trade side.
    pub const fn side(&self) -> Side {
        self.side
    }

    /// Returns the order quantity.
    pub const fn quantity(&self) -> Decimal {
        self.quantity
    }

    /// Returns the order type.
    pub const fn order_type(&self) -> OrderType {
        self.order_type
    }

    /// Returns the optional limit price.
    pub const fn limit_price(&self) -> Option<Decimal> {
        self.limit_price
    }
}

/// Generates an 8-character hex hash from a UUID.
fn generate_hash() -> String {
    Uuid::new_v4().to_string()[..8].to_string()
}

/// A git-style commit of trading actions, bundling multiple staged actions.
#[derive(Debug, Clone, Serialize, Deserialize, Builder)]
pub struct TradingCommit {
    #[builder(default = generate_hash())]
    hash: String,
    #[builder(into)]
    message: String,
    actions: Vec<StagedAction>,
    #[builder(into)]
    strategy_id: String,
    strategy_version: u32,
    #[builder(default = jiff::Timestamp::now())]
    created_at: jiff::Timestamp,
}

impl TradingCommit {
    /// Returns the commit hash (8 characters).
    pub fn hash(&self) -> &str {
        &self.hash
    }

    /// Returns the staged actions in this commit.
    pub fn actions(&self) -> &[StagedAction] {
        &self.actions
    }

    /// Returns the strategy identifier that produced this commit.
    pub fn strategy_id(&self) -> &str {
        &self.strategy_id
    }
}

/// One leg of an arbitrage opportunity.
#[derive(Debug, Clone, Serialize, Deserialize, Builder)]
pub struct ArbLeg {
    #[builder(into)]
    contract_id: String,
    side: Side,
    quantity: Decimal,
    #[builder(into)]
    broker: String,
}

impl ArbLeg {
    /// Returns the contract identifier.
    pub fn contract_id(&self) -> &str {
        &self.contract_id
    }

    /// Returns the trade side.
    pub const fn side(&self) -> Side {
        self.side
    }

    /// Returns the order quantity.
    pub const fn quantity(&self) -> Decimal {
        self.quantity
    }

    /// Returns the broker name.
    pub fn broker(&self) -> &str {
        &self.broker
    }
}

/// A detected arbitrage opportunity across legs.
#[derive(Debug, Clone, Serialize, Deserialize, Builder)]
pub struct ArbOpportunity {
    #[builder(into)]
    strategy_id: String,
    legs: Vec<ArbLeg>,
    expected_spread: Decimal,
    max_slippage: Decimal,
    expiry: jiff::Timestamp,
}

impl ArbOpportunity {
    /// Returns the strategy identifier.
    pub fn strategy_id(&self) -> &str {
        &self.strategy_id
    }

    /// Returns the arbitrage legs.
    pub fn legs(&self) -> &[ArbLeg] {
        &self.legs
    }

    /// Returns the expected spread.
    pub const fn expected_spread(&self) -> Decimal {
        self.expected_spread
    }

    /// Returns the maximum allowed slippage.
    pub const fn max_slippage(&self) -> Decimal {
        self.max_slippage
    }

    /// Returns the expiry timestamp.
    pub const fn expiry(&self) -> jiff::Timestamp {
        self.expiry
    }
}
