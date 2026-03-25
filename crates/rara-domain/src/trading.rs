//! Trading action and commitment types.

use bon::Builder;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use strum::{Display, EnumString};
use uuid::Uuid;

/// Direction of a trade.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Display, EnumString)]
pub enum Side {
    /// Buy / go long.
    Buy,
    /// Sell / go short.
    Sell,
}

/// Type of order to place.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Display, EnumString)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Display, EnumString)]
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
    pub action_type: ActionType,
    #[builder(into)]
    pub contract_id: String,
    pub side: Side,
    pub quantity: Decimal,
    pub order_type: OrderType,
    pub limit_price: Option<Decimal>,
}

/// Generates an 8-character hex hash from a UUID.
fn generate_hash() -> String {
    Uuid::new_v4().to_string()[..8].to_string()
}

/// A git-style commit of trading actions, bundling multiple staged actions.
#[derive(Debug, Clone, Serialize, Deserialize, Builder)]
pub struct TradingCommit {
    #[builder(default = generate_hash())]
    pub hash: String,
    #[builder(into)]
    pub message: String,
    pub actions: Vec<StagedAction>,
    #[builder(into)]
    pub strategy_id: String,
    pub strategy_version: u32,
    #[builder(default = jiff::Timestamp::now())]
    pub created_at: jiff::Timestamp,
}

/// One leg of an arbitrage opportunity.
#[derive(Debug, Clone, Serialize, Deserialize, Builder)]
pub struct ArbLeg {
    #[builder(into)]
    pub contract_id: String,
    pub side: Side,
    pub quantity: Decimal,
    #[builder(into)]
    pub broker: String,
}

/// A detected arbitrage opportunity across legs.
#[derive(Debug, Clone, Serialize, Deserialize, Builder)]
pub struct ArbOpportunity {
    #[builder(into)]
    pub strategy_id: String,
    pub legs: Vec<ArbLeg>,
    pub expected_spread: Decimal,
    pub max_slippage: Decimal,
    pub expiry: jiff::Timestamp,
}
