//! Guard that blocks trading on contracts flagged by critical sentinel signals.

use std::collections::HashSet;

use async_trait::async_trait;
use tokio::sync::RwLock;

use crate::domain::trading::TradingCommit;
use crate::trading::broker::AccountInfo;

use super::{Guard, GuardResult};

/// Rejects commits targeting contracts that have been blocked due to critical
/// sentinel signals.
pub struct SentinelGate {
    /// Contract IDs currently blocked. Updated externally when critical
    /// signals arrive.
    blocked: RwLock<HashSet<String>>,
}

impl SentinelGate {
    /// Create a new sentinel gate with no blocked contracts.
    pub fn new() -> Self {
        Self {
            blocked: RwLock::new(HashSet::new()),
        }
    }

    /// Block trading on the given contract IDs.
    pub async fn block_contracts(&self, contract_ids: impl IntoIterator<Item = String>) {
        let mut blocked = self.blocked.write().await;
        blocked.extend(contract_ids);
    }

    /// Unblock a contract, allowing trading again.
    pub async fn unblock_contract(&self, contract_id: &str) {
        let mut blocked = self.blocked.write().await;
        blocked.remove(contract_id);
    }
}

impl Default for SentinelGate {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Guard for SentinelGate {
    fn name(&self) -> &'static str {
        "SentinelGate"
    }

    async fn check(&self, commit: &TradingCommit, _account: &AccountInfo) -> GuardResult {
        let blocked = self.blocked.read().await;

        for action in commit.actions() {
            if blocked.contains(action.contract_id()) {
                return GuardResult::Reject {
                    reason: format!(
                        "contract {} is blocked by sentinel signal",
                        action.contract_id(),
                    ),
                };
            }
        }

        GuardResult::Allow
    }
}
