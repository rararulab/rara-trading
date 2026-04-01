//! Trading action and commitment types.

pub mod git;
pub mod operation;

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
    /// Action category to execute.
    pub action_type: ActionType,
    /// Target contract identifier.
    #[builder(into)]
    pub contract_id: String,
    /// Order side.
    pub side:        Side,
    /// Order quantity in contract units.
    pub quantity:    Decimal,
    /// Order type semantics.
    pub order_type:  OrderType,
    /// Optional limit/trigger price depending on order type.
    pub limit_price: Option<Decimal>,
}

/// Generates an 8-character hex hash from a UUID.
fn generate_hash() -> String { Uuid::new_v4().to_string()[..8].to_string() }

/// A git-style commit of trading actions, bundling multiple staged actions.
#[derive(Debug, Clone, Serialize, Deserialize, Builder)]
pub struct TradingCommit {
    /// Short commit hash used as correlation key.
    #[builder(default = generate_hash())]
    pub hash:             String,
    /// Commit message describing intent.
    #[builder(into)]
    pub message:          String,
    /// Batched staged actions.
    pub actions:          Vec<StagedAction>,
    /// Strategy that produced this commit.
    #[builder(into)]
    pub strategy_id:      String,
    /// Strategy version used for generation.
    pub strategy_version: u32,
    /// Correlation ID linking this commit to the originating research pipeline
    /// run.
    #[builder(into)]
    pub correlation_id:   Option<String>,
    /// Commit creation timestamp.
    #[builder(default = jiff::Timestamp::now())]
    pub created_at:       jiff::Timestamp,
}

/// One leg of an arbitrage opportunity.
#[derive(Debug, Clone, Serialize, Deserialize, Builder)]
pub struct ArbLeg {
    /// Contract identifier for this leg.
    #[builder(into)]
    pub contract_id: String,
    /// Execution side for this leg.
    pub side:        Side,
    /// Leg quantity.
    pub quantity:    Decimal,
    /// Broker/exchange route for this leg.
    #[builder(into)]
    pub broker:      String,
}

/// A detected arbitrage opportunity across legs.
#[derive(Debug, Clone, Serialize, Deserialize, Builder)]
pub struct ArbOpportunity {
    /// Strategy that discovered the opportunity.
    #[builder(into)]
    pub strategy_id:     String,
    /// Arbitrage legs to execute atomically.
    pub legs:            Vec<ArbLeg>,
    /// Expected gross spread across legs.
    pub expected_spread: Decimal,
    /// Maximum tolerated execution slippage.
    pub max_slippage:    Decimal,
    /// Opportunity expiry timestamp.
    pub expiry:          jiff::Timestamp,
}
