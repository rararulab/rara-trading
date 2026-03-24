//! Guard that rejects orders targeting contracts not in a whitelist.

use std::collections::HashSet;

use async_trait::async_trait;

use rara_domain::trading::TradingCommit;
use crate::broker::AccountInfo;

use super::{Guard, GuardResult};

/// Rejects commits containing actions on contracts not present in the allowed
/// set.
pub struct SymbolWhitelist {
    /// Set of allowed contract identifiers.
    allowed: HashSet<String>,
}

impl SymbolWhitelist {
    /// Create a new whitelist guard from an iterator of allowed contract IDs.
    pub fn new(allowed: impl IntoIterator<Item = String>) -> Self {
        Self {
            allowed: allowed.into_iter().collect(),
        }
    }
}

#[async_trait]
impl Guard for SymbolWhitelist {
    fn name(&self) -> &'static str {
        "SymbolWhitelist"
    }

    async fn check(&self, commit: &TradingCommit, _account: &AccountInfo) -> GuardResult {
        for action in commit.actions() {
            if !self.allowed.contains(action.contract_id()) {
                return GuardResult::Reject {
                    reason: format!(
                        "contract {} is not in the symbol whitelist",
                        action.contract_id(),
                    ),
                };
            }
        }

        GuardResult::Allow
    }
}
