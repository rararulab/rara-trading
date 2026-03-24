//! Sequential guard pipeline that short-circuits on the first rejection.

use crate::domain::trading::TradingCommit;
use crate::trading::broker::AccountInfo;
use crate::trading::guards::{Guard, GuardResult};

/// Runs a sequence of guards, stopping at the first rejection.
pub struct GuardPipeline {
    /// Ordered list of guards to evaluate.
    guards: Vec<Box<dyn Guard>>,
}

impl GuardPipeline {
    /// Create a new pipeline from the given guards.
    pub fn new(guards: Vec<Box<dyn Guard>>) -> Self {
        Self { guards }
    }

    /// Run all guards in order. Returns the first rejection, or `Allow` if
    /// all guards pass.
    pub async fn run(&self, commit: &TradingCommit, account: &AccountInfo) -> GuardResult {
        for guard in &self.guards {
            let result = guard.check(commit, account).await;
            if matches!(result, GuardResult::Reject { .. }) {
                return result;
            }
        }

        GuardResult::Allow
    }
}

#[cfg(test)]
mod tests {
    use rust_decimal::Decimal;

    use crate::domain::trading::{ActionType, OrderType, Side, StagedAction, TradingCommit};
    use crate::trading::broker::AccountInfo;
    use crate::trading::guards::symbol_whitelist::SymbolWhitelist;

    use super::*;

    fn test_commit() -> TradingCommit {
        TradingCommit::builder()
            .message("test commit")
            .strategy_id("strat-1")
            .strategy_version(1)
            .actions(vec![StagedAction::builder()
                .action_type(ActionType::PlaceOrder)
                .contract_id("BTC-USD")
                .side(Side::Buy)
                .quantity(Decimal::ONE)
                .order_type(OrderType::Market)
                .build()])
            .build()
    }

    fn test_account() -> AccountInfo {
        AccountInfo::builder()
            .total_equity(Decimal::new(100_000, 0))
            .available_cash(Decimal::new(50_000, 0))
            .positions(vec![])
            .build()
    }

    #[tokio::test]
    async fn pipeline_allows_when_all_guards_pass() {
        let pipeline = GuardPipeline::new(vec![Box::new(SymbolWhitelist::new(
            vec!["BTC-USD".to_string()],
        ))]);

        let result = pipeline.run(&test_commit(), &test_account()).await;
        assert!(matches!(result, GuardResult::Allow));
    }

    #[tokio::test]
    async fn pipeline_rejects_on_first_failure() {
        // First guard allows, second rejects (symbol not whitelisted)
        let pipeline = GuardPipeline::new(vec![
            Box::new(SymbolWhitelist::new(vec!["BTC-USD".to_string()])),
            Box::new(SymbolWhitelist::new(vec!["ETH-USD".to_string()])),
        ]);

        let result = pipeline.run(&test_commit(), &test_account()).await;
        assert!(matches!(result, GuardResult::Reject { .. }));
    }

    #[tokio::test]
    async fn empty_pipeline_allows() {
        let pipeline = GuardPipeline::new(vec![]);
        let result = pipeline.run(&test_commit(), &test_account()).await;
        assert!(matches!(result, GuardResult::Allow));
    }
}
