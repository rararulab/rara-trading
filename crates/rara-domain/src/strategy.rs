//! Strategy, strategy classification, and risk profile models.

use bon::Builder;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::contract::SecType;

/// Classification of trading strategy approaches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
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
    id: String,
    version: u32,
    #[builder(into)]
    name: String,
    #[builder(into)]
    description: String,
    #[builder(into)]
    code: String,
    strategy_type: StrategyType,
    applicable_contracts: Vec<ContractFilter>,
    parameters: serde_json::Value,
    status: StrategyStatus,
}

impl Strategy {
    /// Returns the strategy identifier.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Returns the strategy version number.
    pub const fn version(&self) -> u32 {
        self.version
    }

    /// Returns the human-readable strategy name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the current lifecycle status.
    pub const fn status(&self) -> StrategyStatus {
        self.status
    }

    /// Returns the strategy type classification.
    pub const fn strategy_type(&self) -> StrategyType {
        self.strategy_type
    }

    /// Returns the contract filters this strategy applies to.
    pub fn applicable_contracts(&self) -> &[ContractFilter] {
        &self.applicable_contracts
    }
}

/// Risk limits and constraints for a specific security type.
#[derive(Debug, Clone, Serialize, Deserialize, Builder)]
pub struct RiskProfile {
    sec_type: SecType,
    max_leverage: Decimal,
    max_position_pct: Decimal,
    max_drawdown: Decimal,
    require_stop_loss: bool,
    liquidation_buffer: Decimal,
    funding_rate_check: bool,
}

impl RiskProfile {
    /// Returns the maximum allowed leverage.
    pub const fn max_leverage(&self) -> Decimal {
        self.max_leverage
    }

    /// Returns whether a stop-loss is required.
    pub const fn require_stop_loss(&self) -> bool {
        self.require_stop_loss
    }

    /// Returns whether funding rate checks are enabled.
    pub const fn funding_rate_check(&self) -> bool {
        self.funding_rate_check
    }

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
