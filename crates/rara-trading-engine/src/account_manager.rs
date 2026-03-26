//! Multi-account registry and lifecycle manager.
//!
//! [`AccountManager`] holds multiple [`UnifiedTradingAccount`] instances and
//! provides lookup, resolution, and aggregated equity queries.

use std::collections::HashMap;

use bon::Builder;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use snafu::Snafu;

use crate::account_config::AccountConfig;
use crate::health::{BrokerHealth, BrokerHealthInfo};
use crate::uta::UnifiedTradingAccount;

/// Summary of a registered account.
#[derive(Debug, Clone, Serialize, Deserialize, Builder)]
pub struct AccountSummary {
    /// Account identifier.
    #[builder(into)]
    pub id: String,
    /// Human-readable label.
    #[builder(into)]
    pub label: String,
    /// Current health snapshot.
    pub health: BrokerHealthInfo,
}

/// Aggregated equity across all accounts.
#[derive(Debug, Clone, Serialize, Deserialize, Builder)]
pub struct AggregatedEquity {
    /// Sum of equity from all healthy accounts.
    pub total_equity: Decimal,
    /// Sum of cash from all healthy accounts.
    pub total_cash: Decimal,
    /// Sum of unrealized P&L from all healthy accounts.
    pub total_unrealized_pnl: Decimal,
    /// Per-account equity breakdown.
    pub accounts: Vec<AccountEquity>,
}

/// Single account equity entry within an aggregated view.
#[derive(Debug, Clone, Serialize, Deserialize, Builder)]
pub struct AccountEquity {
    /// Account identifier.
    #[builder(into)]
    pub id: String,
    /// Human-readable label.
    #[builder(into)]
    pub label: String,
    /// Account equity (zero if offline).
    pub equity: Decimal,
    /// Account cash (zero if offline).
    pub cash: Decimal,
    /// Unrealized P&L (zero if offline).
    pub unrealized_pnl: Decimal,
    /// Health status at the time of query.
    pub health: BrokerHealth,
}

/// Error from account manager operations.
#[derive(Debug, Snafu)]
#[snafu(visibility(pub))]
pub enum AccountManagerError {
    /// The requested account was not found.
    #[snafu(display("account not found: {id}"))]
    NotFound {
        /// The ID that was looked up.
        id: String,
    },
    /// The source string matched multiple accounts.
    #[snafu(display("ambiguous source '{query}': matched {count} accounts"))]
    Ambiguous {
        /// The query string that was ambiguous.
        query: String,
        /// Number of accounts matched.
        count: usize,
    },
}

/// Multi-account registry that owns [`UnifiedTradingAccount`] instances.
///
/// Supports lookup by exact ID, substring matching on id/label,
/// and aggregated equity queries across all registered accounts.
pub struct AccountManager {
    accounts: HashMap<String, UnifiedTradingAccount>,
}

impl AccountManager {
    /// Build an `AccountManager` from declarative account configs.
    ///
    /// Only enabled accounts are initialized. Disabled accounts are skipped.
    pub fn from_config(accounts: &[AccountConfig]) -> Result<Self, AccountManagerError> {
        let mut mgr = Self::new();
        for acc in accounts.iter().filter(|a| a.enabled) {
            let broker = acc.broker_config.create_broker();
            let label = acc.label.as_deref().unwrap_or(&acc.id);
            mgr.add(UnifiedTradingAccount::new(&acc.id, label, broker));
        }
        Ok(mgr)
    }

    /// Create an empty account manager.
    pub fn new() -> Self {
        Self {
            accounts: HashMap::new(),
        }
    }

    /// Register a UTA. Uses `uta.id` as the key, replacing any previous entry.
    pub fn add(&mut self, uta: UnifiedTradingAccount) {
        self.accounts.insert(uta.id.clone(), uta);
    }

    /// Remove and return a UTA by its ID.
    pub fn remove(&mut self, id: &str) -> Option<UnifiedTradingAccount> {
        self.accounts.remove(id)
    }

    /// Look up a UTA by exact ID.
    pub fn get(&self, id: &str) -> Option<&UnifiedTradingAccount> {
        self.accounts.get(id)
    }

    /// Check whether an account with the given ID is registered.
    pub fn has(&self, id: &str) -> bool {
        self.accounts.contains_key(id)
    }

    /// Return the number of registered accounts.
    pub fn size(&self) -> usize {
        self.accounts.len()
    }

    /// Return `(id, label)` pairs for all registered accounts.
    pub fn list(&self) -> Vec<(&str, &str)> {
        let mut pairs: Vec<_> = self
            .accounts
            .values()
            .map(|uta| (uta.id.as_str(), uta.label.as_str()))
            .collect();
        pairs.sort_by_key(|(id, _)| *id);
        pairs
    }

    /// Resolve accounts matching an optional source string.
    ///
    /// - `source=None` returns all accounts.
    /// - Exact ID match is tried first.
    /// - Falls back to substring match on id or label.
    pub fn resolve(&self, source: Option<&str>) -> Vec<&UnifiedTradingAccount> {
        let Some(source) = source else {
            let mut all: Vec<_> = self.accounts.values().collect();
            all.sort_by_key(|uta| &uta.id);
            return all;
        };

        // Exact ID match first
        if let Some(uta) = self.accounts.get(source) {
            return vec![uta];
        }

        // Substring match on id or label
        let mut matches: Vec<_> = self
            .accounts
            .values()
            .filter(|uta| uta.id.contains(source) || uta.label.contains(source))
            .collect();
        matches.sort_by_key(|uta| &uta.id);
        matches
    }

    /// Resolve exactly one account from a source string.
    ///
    /// Returns an error if zero or more than one account matches.
    pub fn resolve_one(&self, source: &str) -> Result<&UnifiedTradingAccount, AccountManagerError> {
        let matches = self.resolve(Some(source));
        match matches.len() {
            0 => NotFoundSnafu { id: source }.fail(),
            1 => Ok(matches[0]),
            n => AmbiguousSnafu {
                query: source,
                count: n,
            }
            .fail(),
        }
    }

    /// Compute aggregated equity across all registered accounts.
    ///
    /// Offline accounts contribute zero equity but still appear in the
    /// per-account breakdown so the caller can see their health status.
    pub async fn aggregated_equity(&self) -> AggregatedEquity {
        let mut total_equity = Decimal::ZERO;
        let mut total_cash = Decimal::ZERO;
        let mut total_unrealized_pnl = Decimal::ZERO;
        let mut accounts = Vec::with_capacity(self.accounts.len());

        // Sort by ID for deterministic output
        let mut sorted: Vec<_> = self.accounts.values().collect();
        sorted.sort_by_key(|uta| &uta.id);

        for uta in sorted {
            let health = uta.health().await;

            let (equity, cash, unrealized_pnl) = if health == BrokerHealth::Offline {
                (Decimal::ZERO, Decimal::ZERO, Decimal::ZERO)
            } else {
                match uta.get_account().await {
                    Ok(info) => {
                        let unrealized: Decimal =
                            info.positions.iter().map(|p| p.unrealized_pnl).sum();
                        (info.total_equity, info.available_cash, unrealized)
                    }
                    Err(_) => (Decimal::ZERO, Decimal::ZERO, Decimal::ZERO),
                }
            };

            total_equity += equity;
            total_cash += cash;
            total_unrealized_pnl += unrealized_pnl;

            accounts.push(AccountEquity {
                id: uta.id.clone(),
                label: uta.label.clone(),
                equity,
                cash,
                unrealized_pnl,
                health,
            });
        }

        AggregatedEquity {
            total_equity,
            total_cash,
            total_unrealized_pnl,
            accounts,
        }
    }
}

impl Default for AccountManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use rust_decimal::Decimal;
    use rust_decimal_macros::dec;

    use crate::account_config::{BrokerConfig, PaperBrokerConfig};
    use crate::brokers::paper::PaperBroker;
    use crate::uta::UnifiedTradingAccount;

    use super::*;

    fn make_uta(id: &str) -> UnifiedTradingAccount {
        let broker = PaperBroker::new(Decimal::new(100_000, 0));
        UnifiedTradingAccount::new(id, format!("{id} label"), Box::new(broker))
    }

    #[test]
    fn add_and_get() {
        let mut mgr = AccountManager::new();
        assert_eq!(mgr.size(), 0);
        assert!(!mgr.has("acct-1"));

        mgr.add(make_uta("acct-1"));
        assert_eq!(mgr.size(), 1);
        assert!(mgr.has("acct-1"));

        let uta = mgr.get("acct-1").expect("should find acct-1");
        assert_eq!(uta.id, "acct-1");
        assert_eq!(uta.label, "acct-1 label");
    }

    #[test]
    fn remove_account() {
        let mut mgr = AccountManager::new();
        mgr.add(make_uta("acct-1"));
        mgr.add(make_uta("acct-2"));
        assert_eq!(mgr.size(), 2);

        let removed = mgr.remove("acct-1").expect("should remove acct-1");
        assert_eq!(removed.id, "acct-1");
        assert_eq!(mgr.size(), 1);
        assert!(!mgr.has("acct-1"));

        // Removing non-existent returns None
        assert!(mgr.remove("acct-1").is_none());
    }

    #[test]
    fn list_accounts() {
        let mut mgr = AccountManager::new();
        mgr.add(make_uta("beta"));
        mgr.add(make_uta("alpha"));

        let list = mgr.list();
        // Should be sorted by ID
        assert_eq!(list, vec![("alpha", "alpha label"), ("beta", "beta label")]);
    }

    #[test]
    fn resolve_one_succeeds() {
        let mut mgr = AccountManager::new();
        mgr.add(make_uta("prod-binance"));
        mgr.add(make_uta("paper-bybit"));

        // Exact match
        let uta = mgr.resolve_one("prod-binance").expect("exact match");
        assert_eq!(uta.id, "prod-binance");

        // Substring match (unique)
        let uta = mgr.resolve_one("bybit").expect("substring match");
        assert_eq!(uta.id, "paper-bybit");
    }

    #[test]
    fn resolve_one_fails_on_unknown() {
        let mgr = AccountManager::new();
        match mgr.resolve_one("nonexistent") {
            Err(e) => assert!(e.to_string().contains("not found")),
            Ok(_) => panic!("expected NotFound error"),
        }
    }

    #[test]
    fn resolve_one_fails_on_ambiguous() {
        let mut mgr = AccountManager::new();
        mgr.add(make_uta("prod-binance"));
        mgr.add(make_uta("paper-binance"));

        match mgr.resolve_one("binance") {
            Err(e) => {
                assert!(e.to_string().contains("ambiguous"));
                assert!(e.to_string().contains('2'));
            }
            Ok(_) => panic!("expected Ambiguous error"),
        }
    }

    #[tokio::test]
    async fn aggregated_equity() {
        let mut mgr = AccountManager::new();
        mgr.add(make_uta("acct-1"));
        mgr.add(make_uta("acct-2"));

        let agg = mgr.aggregated_equity().await;

        // PaperBroker returns 100_000 equity per account
        assert_eq!(agg.total_equity, dec!(200_000));
        assert_eq!(agg.total_cash, dec!(200_000));
        assert_eq!(agg.total_unrealized_pnl, dec!(0));
        assert_eq!(agg.accounts.len(), 2);

        // Each account should report its own equity
        for entry in &agg.accounts {
            assert_eq!(entry.equity, dec!(100_000));
            assert_eq!(entry.health, BrokerHealth::Healthy);
        }
    }

    #[test]
    fn from_config_creates_enabled_accounts() {
        let accounts = vec![
            AccountConfig {
                id: "paper-1".to_string(),
                label: Some("Paper One".to_string()),
                broker_config: BrokerConfig::Paper(PaperBrokerConfig {
                    fill_price: Some(100.0),
                }),
                enabled: true,
                contracts: vec!["BTC-USDT".to_string()],
            },
            AccountConfig {
                id: "paper-2".to_string(),
                label: None,
                broker_config: BrokerConfig::Paper(PaperBrokerConfig {
                    fill_price: None,
                }),
                enabled: false,
                contracts: vec![],
            },
        ];
        let mgr = AccountManager::from_config(&accounts).unwrap();
        assert_eq!(mgr.size(), 1);
        assert!(mgr.has("paper-1"));
        assert!(!mgr.has("paper-2"));
    }

    #[test]
    fn from_config_uses_label() {
        let accounts = vec![AccountConfig {
            id: "test".to_string(),
            label: Some("My Label".to_string()),
            broker_config: BrokerConfig::Paper(PaperBrokerConfig { fill_price: None }),
            enabled: true,
            contracts: vec![],
        }];
        let mgr = AccountManager::from_config(&accounts).unwrap();
        let list = mgr.list();
        assert_eq!(list[0].1, "My Label");
    }

    #[test]
    fn from_config_empty() {
        let mgr = AccountManager::from_config(&[]).unwrap();
        assert_eq!(mgr.size(), 0);
    }
}
