//! Strategy, strategy classification, and risk profile models.

use bon::Builder;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use strum::{Display, EnumString};

use crate::contract::SecType;

/// Classification of trading strategy approaches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Display, EnumString)]
pub enum StrategyType {
    /// Directional bets on price movement.
    Directional,
    /// Arbitrage across exchanges.
    CrossExchangeArb,
    /// Statistical pairs trading.
    PairsTrading,
    /// Prediction market arbitrage.
    PredictionArb,
    /// Basis (cash-and-carry) arbitrage.
    BasisArb,
}

/// Lifecycle status of a strategy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Display, EnumString)]
pub enum StrategyStatus {
    /// Newly identified, not yet tested.
    Candidate,
    /// Currently being backtested.
    Backtesting,
    /// Running on paper/simulated trading.
    PaperTrading,
    /// Deployed with real capital.
    Live,
    /// No longer active.
    Retired,
}

/// Filter for matching contracts to strategies.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ContractFilter {
    /// Match a specific contract by ID.
    Exact(String),
    /// Match all contracts of a given security type.
    BySecType(SecType),
    /// Match all contracts on a given exchange.
    ByExchange(String),
    /// Custom filter expression.
    Custom(String),
}

/// A trading strategy with its configuration and lifecycle status.
#[derive(Debug, Clone, Serialize, Deserialize, Builder)]
#[allow(clippy::struct_field_names)]
pub struct Strategy {
    /// Stable strategy identifier.
    #[builder(into)]
    pub id: String,
    /// Monotonic strategy version.
    pub version: u32,
    /// Human-readable strategy name.
    #[builder(into)]
    pub name: String,
    /// Strategy description and intent.
    #[builder(into)]
    pub description: String,
    /// Executable strategy source code.
    #[builder(into)]
    pub code: String,
    /// Strategy classification.
    pub strategy_type: StrategyType,
    /// Contract matching rules this strategy applies to.
    pub applicable_contracts: Vec<ContractFilter>,
    /// JSON parameters consumed by strategy runtime.
    pub parameters: serde_json::Value,
    /// Lifecycle status of the strategy.
    pub status: StrategyStatus,
}

/// Risk limits and constraints for a specific security type.
#[derive(Debug, Clone, Serialize, Deserialize, Builder)]
pub struct RiskProfile {
    /// Security type this risk profile targets.
    pub sec_type: SecType,
    /// Maximum leverage allowed.
    pub max_leverage: Decimal,
    /// Maximum position size as portfolio fraction.
    pub max_position_pct: Decimal,
    /// Maximum tolerated drawdown.
    pub max_drawdown: Decimal,
    /// Whether stop-loss orders are mandatory.
    pub require_stop_loss: bool,
    /// Minimum liquidation safety buffer.
    pub liquidation_buffer: Decimal,
    /// Whether funding-rate checks are required.
    pub funding_rate_check: bool,
}

impl RiskProfile {
    /// Default risk profile for crypto spot trading.
    pub fn crypto_spot_default() -> Self {
        Self {
            sec_type: SecType::CryptoSpot,
            max_leverage: Decimal::from(1),
            max_position_pct: Decimal::new(10, 2), // 0.10
            max_drawdown: Decimal::new(5, 2),       // 0.05
            require_stop_loss: false,
            liquidation_buffer: Decimal::ZERO,
            funding_rate_check: false,
        }
    }

    /// Default risk profile for crypto perpetual swaps.
    pub fn crypto_perp_default() -> Self {
        Self {
            sec_type: SecType::CryptoPerp,
            max_leverage: Decimal::from(5),
            max_position_pct: Decimal::new(5, 2),   // 0.05
            max_drawdown: Decimal::new(10, 2),       // 0.10
            require_stop_loss: true,
            liquidation_buffer: Decimal::new(2, 2),  // 0.02
            funding_rate_check: true,
        }
    }
}
