//! Contract and security type definitions.

use bon::Builder;
use serde::{Deserialize, Serialize};
use strum::{Display, EnumString};

/// Classification of tradable security types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Display, EnumString)]
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
    /// Exchange identifier, e.g. `"binance"`.
    #[builder(into)]
    pub exchange: String,
    /// Exchange-native symbol, e.g. `"BTCUSDT"`.
    #[builder(into)]
    pub symbol: String,
    /// Security type classification.
    pub sec_type: SecType,
    /// Quote currency code, e.g. `"USDT"`.
    #[builder(into)]
    pub currency: String,
}

impl Contract {
    /// Returns a unique identifier in the format `"{exchange}-{symbol}"`.
    pub fn id(&self) -> String {
        format!("{}-{}", self.exchange, self.symbol)
    }
}
