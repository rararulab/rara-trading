//! Guard trait and built-in guard implementations for pre-trade risk checks.

pub mod drawdown_limit;
pub mod max_position_size;
pub mod sentinel_gate;
pub mod symbol_whitelist;

use async_trait::async_trait;

use crate::domain::trading::TradingCommit;
use crate::trading::broker::AccountInfo;

/// Outcome of a guard check.
#[derive(Debug, Clone)]
pub enum GuardResult {
    /// The commit is allowed to proceed.
    Allow,
    /// The commit is rejected with a reason.
    Reject {
        /// Human-readable explanation for the rejection.
        reason: String,
    },
}

/// Pre-trade risk check that can allow or reject a trading commit.
#[async_trait]
pub trait Guard: Send + Sync {
    /// Human-readable name of this guard.
    fn name(&self) -> &'static str;

    /// Evaluate whether the commit should be allowed given the current account
    /// state.
    async fn check(&self, commit: &TradingCommit, account: &AccountInfo) -> GuardResult;
}
