//! Contract and security type definitions.

use bon::Builder;
use serde::{Deserialize, Serialize};

/// Classification of tradable security types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SecType {
    /// Spot cryptocurrency — no leverage.
    CryptoSpot,
    /// Perpetual swap — leveraged, funding rate.
    CryptoPerp,
    /// Delivery futures — leveraged, expiry.
    CryptoFuture,
    /// Equities.
    Stock,
    /// Traditional futures.
    Future,
    /// Options contracts.
    Option,
    /// Prediction markets (e.g. Polymarket).
    Prediction,
}

/// A tradable instrument on a specific exchange.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Builder)]
pub struct Contract {
    #[builder(into)]
    exchange: String,
    #[builder(into)]
    symbol: String,
    sec_type: SecType,
    #[builder(into)]
    currency: String,
}

impl Contract {
    /// Returns a unique identifier in the format `"{exchange}-{symbol}"`.
    pub fn id(&self) -> String {
        format!("{}-{}", self.exchange, self.symbol)
    }

    /// Returns the exchange name.
    pub fn exchange(&self) -> &str {
        &self.exchange
    }

    /// Returns the trading symbol.
    pub fn symbol(&self) -> &str {
        &self.symbol
    }

    /// Returns the security type.
    pub const fn sec_type(&self) -> SecType {
        self.sec_type
    }

    /// Returns the quote currency.
    pub fn currency(&self) -> &str {
        &self.currency
    }
}
