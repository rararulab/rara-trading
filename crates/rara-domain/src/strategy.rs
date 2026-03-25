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
    #[builder(into)]
    pub id: String,
    pub version: u32,
    #[builder(into)]
    pub name: String,
    #[builder(into)]
    pub description: String,
    #[builder(into)]
    pub code: String,
    pub strategy_type: StrategyType,
    pub applicable_contracts: Vec<ContractFilter>,
    pub parameters: serde_json::Value,
    pub status: StrategyStatus,
}

/// Risk limits and constraints for a specific security type.
#[derive(Debug, Clone, Serialize, Deserialize, Builder)]
pub struct RiskProfile {
    pub sec_type: SecType,
    pub max_leverage: Decimal,
    pub max_position_pct: Decimal,
    pub max_drawdown: Decimal,
    pub require_stop_loss: bool,
    pub liquidation_buffer: Decimal,
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
