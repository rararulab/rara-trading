//! Strategy-to-contract binding configuration.

use bon::Builder;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// Operational mode for a strategy binding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TradingMode {
    /// Simulated execution — no real orders placed.
    Paper,
    /// Live execution against a real broker.
    Live,
}

/// Binds a strategy version to a specific contract with capital allocation and
/// mode settings.
#[derive(Debug, Clone, Serialize, Deserialize, Builder)]
pub struct StrategyBinding {
    /// Strategy identifier.
    #[builder(into)]
    strategy_id:       String,
    /// Strategy version number.
    strategy_version:  u32,
    /// Target contract identifier.
    #[builder(into)]
    contract_id:       String,
    /// Paper or live trading mode.
    mode:              TradingMode,
    /// Capital allocated to this binding.
    allocated_capital: Decimal,
    /// When this binding was activated.
    activated_at:      jiff::Timestamp,
}

impl StrategyBinding {
    /// Returns the strategy identifier.
    pub fn strategy_id(&self) -> &str { &self.strategy_id }

    /// Returns the strategy version.
    pub const fn strategy_version(&self) -> u32 { self.strategy_version }

    /// Returns the target contract identifier.
    pub fn contract_id(&self) -> &str { &self.contract_id }

    /// Returns the trading mode.
    pub const fn mode(&self) -> &TradingMode { &self.mode }

    /// Returns the allocated capital.
    pub const fn allocated_capital(&self) -> Decimal { self.allocated_capital }

    /// Returns the activation timestamp.
    pub const fn activated_at(&self) -> jiff::Timestamp { self.activated_at }
}
