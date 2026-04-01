//! Declarative account configuration for `accounts.toml`.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Root structure for `accounts.toml`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AccountsConfig {
    /// List of trading accounts.
    #[serde(default)]
    pub accounts: Vec<AccountConfig>,
}

/// A single trading account definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountConfig {
    /// Unique account identifier.
    pub id:            String,
    /// Human-readable label.
    pub label:         Option<String>,
    /// Broker-specific configuration (discriminated by `broker` field).
    #[serde(flatten)]
    pub broker_config: BrokerConfig,
    /// Whether the account is active.
    #[serde(default = "default_true")]
    pub enabled:       bool,
    /// Contracts to trade on this account.
    #[serde(default)]
    pub contracts:     Vec<String>,
}

impl AccountConfig {
    /// Replace sensitive fields with "****".
    pub fn mask_secrets(&mut self) {
        let BrokerConfig::Ccxt(ref mut c) = self.broker_config;
        if !c.api_key.is_empty() {
            c.api_key = "****".to_string();
        }
        if !c.secret.is_empty() {
            c.secret = "****".to_string();
        }
        if c.passphrase.is_some() {
            c.passphrase = Some("****".to_string());
        }
    }
}

/// Broker-specific configuration, discriminated by the `broker` field.
///
/// Only real exchange brokers are supported — paper/simulated brokers are
/// restricted to the test suite (`brokers::paper::PaperBroker`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "broker", content = "broker_config", rename_all = "snake_case")]
pub enum BrokerConfig {
    /// CCXT-based exchange (Binance, Bybit, OKX).
    /// Use `sandbox = true` for testnet/paper trading.
    Ccxt(CcxtBrokerConfig),
}

impl BrokerConfig {
    /// Return the broker type key for registry lookup.
    pub const fn type_key(&self) -> &str {
        match self {
            Self::Ccxt(_) => "ccxt",
        }
    }

    /// Convert broker config into a flat field map for the registry factory.
    pub fn to_field_map(&self) -> HashMap<String, String> {
        let mut m = HashMap::new();
        match self {
            Self::Ccxt(c) => {
                m.insert("exchange".to_string(), c.exchange.clone());
                m.insert("sandbox".to_string(), c.sandbox.to_string());
                m.insert("api_key".to_string(), c.api_key.clone());
                m.insert("secret".to_string(), c.secret.clone());
                if let Some(ref p) = c.passphrase {
                    m.insert("passphrase".to_string(), p.clone());
                }
            }
        }
        m
    }
}

/// CCXT broker configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CcxtBrokerConfig {
    /// Exchange identifier: "binance", "bybit", "okx".
    pub exchange:   String,
    /// Use exchange sandbox/testnet.
    #[serde(default)]
    pub sandbox:    bool,
    /// API key (sensitive — masked in CLI output).
    #[serde(default)]
    pub api_key:    String,
    /// API secret (sensitive — masked in CLI output).
    #[serde(default)]
    pub secret:     String,
    /// API passphrase (OKX only, sensitive).
    pub passphrase: Option<String>,
}

const fn default_true() -> bool { true }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserialize_ccxt_account() {
        let toml_str = r#"
            [[accounts]]
            id = "binance-prod"
            broker = "ccxt"
            contracts = ["BTC-USDT", "ETH-USDT"]

            [accounts.broker_config]
            exchange = "binance"
            sandbox = true
            api_key = "key123"
            secret = "secret456"
        "#;
        let cfg: AccountsConfig = toml::from_str(toml_str).unwrap();
        let acc = &cfg.accounts[0];
        assert!(matches!(&acc.broker_config, BrokerConfig::Ccxt(c) if c.exchange == "binance"));
    }

    #[test]
    fn deserialize_multiple_accounts() {
        let toml_str = r#"
            [[accounts]]
            id = "sandbox"
            broker = "ccxt"

            [accounts.broker_config]
            exchange = "binance"
            sandbox = true

            [[accounts]]
            id = "live"
            broker = "ccxt"

            [accounts.broker_config]
            exchange = "bybit"
        "#;
        let cfg: AccountsConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(cfg.accounts.len(), 2);
    }

    #[test]
    fn defaults_applied() {
        let toml_str = r#"
            [[accounts]]
            id = "minimal"
            broker = "ccxt"

            [accounts.broker_config]
            exchange = "binance"
        "#;
        let cfg: AccountsConfig = toml::from_str(toml_str).unwrap();
        let acc = &cfg.accounts[0];
        assert!(acc.enabled);
        assert!(acc.contracts.is_empty());
        assert!(acc.label.is_none());
    }

    #[test]
    fn serialize_roundtrip() {
        let cfg = AccountsConfig {
            accounts: vec![AccountConfig {
                id:            "test".to_string(),
                label:         Some("Test Account".to_string()),
                broker_config: BrokerConfig::Ccxt(CcxtBrokerConfig {
                    exchange:   "binance".to_string(),
                    sandbox:    true,
                    api_key:    String::new(),
                    secret:     String::new(),
                    passphrase: None,
                }),
                enabled:       true,
                contracts:     vec!["BTC-USDT".to_string()],
            }],
        };
        let serialized = toml::to_string_pretty(&cfg).unwrap();
        let deserialized: AccountsConfig = toml::from_str(&serialized).unwrap();
        assert_eq!(deserialized.accounts.len(), 1);
        assert_eq!(deserialized.accounts[0].id, "test");
    }

    #[test]
    fn mask_secrets_hides_sensitive_fields() {
        let mut acc = AccountConfig {
            id:            "test".to_string(),
            label:         None,
            broker_config: BrokerConfig::Ccxt(CcxtBrokerConfig {
                exchange:   "binance".to_string(),
                sandbox:    false,
                api_key:    "my-secret-key".to_string(),
                secret:     "my-secret-value".to_string(),
                passphrase: Some("my-pass".to_string()),
            }),
            enabled:       true,
            contracts:     vec![],
        };
        acc.mask_secrets();
        let BrokerConfig::Ccxt(ref c) = acc.broker_config;
        assert_eq!(c.api_key, "****");
        assert_eq!(c.secret, "****");
        assert_eq!(c.passphrase.as_deref(), Some("****"));
    }
}
