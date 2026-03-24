//! Guard that rejects trading when account equity has dropped below a
//! threshold from its peak.

use async_trait::async_trait;
use rust_decimal::Decimal;

use crate::domain::trading::TradingCommit;
use crate::trading::broker::AccountInfo;

use super::{Guard, GuardResult};

/// Rejects commits when current equity is below `(1 - max_drawdown_pct) *
/// peak_equity`.
pub struct DrawdownLimit {
    /// Peak equity observed (set externally or tracked over time).
    peak_equity: Decimal,
    /// Maximum allowed drawdown as a fraction (e.g. 0.20 = 20%).
    max_drawdown_pct: Decimal,
}

impl DrawdownLimit {
    /// Create a new drawdown limit guard.
    pub const fn new(peak_equity: Decimal, max_drawdown_pct: Decimal) -> Self {
        Self {
            peak_equity,
            max_drawdown_pct,
        }
    }
}

#[async_trait]
impl Guard for DrawdownLimit {
    fn name(&self) -> &'static str {
        "DrawdownLimit"
    }

    async fn check(&self, _commit: &TradingCommit, account: &AccountInfo) -> GuardResult {
        let threshold = self.peak_equity * (Decimal::ONE - self.max_drawdown_pct);

        if account.total_equity < threshold {
            return GuardResult::Reject {
                reason: format!(
                    "equity {} is below drawdown threshold {threshold} \
                     (peak: {}, max drawdown: {})",
                    account.total_equity, self.peak_equity, self.max_drawdown_pct,
                ),
            };
        }

        GuardResult::Allow
    }
}
