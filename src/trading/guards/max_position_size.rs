//! Guard that rejects orders exceeding a maximum position size as a percentage
//! of total equity.

use async_trait::async_trait;
use rust_decimal::Decimal;

use crate::domain::trading::TradingCommit;
use crate::trading::broker::AccountInfo;

use super::{Guard, GuardResult};

/// Rejects commits where any single action's notional value exceeds a maximum
/// percentage of total equity.
pub struct MaxPositionSize {
    /// Maximum allowed position size as a fraction (e.g. 0.10 = 10%).
    max_pct: Decimal,
    /// Assumed price for notional calculation. In production this would come
    /// from a market data feed; here we use a fixed estimate.
    estimated_price: Decimal,
}

impl MaxPositionSize {
    /// Create a new guard with the given maximum percentage and estimated
    /// price.
    pub const fn new(max_pct: Decimal, estimated_price: Decimal) -> Self {
        Self {
            max_pct,
            estimated_price,
        }
    }
}

#[async_trait]
impl Guard for MaxPositionSize {
    fn name(&self) -> &'static str {
        "MaxPositionSize"
    }

    async fn check(&self, commit: &TradingCommit, account: &AccountInfo) -> GuardResult {
        let max_notional = account.total_equity * self.max_pct;

        for action in commit.actions() {
            let notional = action.quantity() * self.estimated_price;
            if notional > max_notional {
                return GuardResult::Reject {
                    reason: format!(
                        "action on {} has notional {notional} exceeding max {max_notional} \
                         ({} of equity)",
                        action.contract_id(),
                        self.max_pct,
                    ),
                };
            }
        }

        GuardResult::Allow
    }
}
