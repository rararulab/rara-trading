//! Broker registry — self-describing broker types for dynamic UI generation.
//!
//! Each broker registers a [`BrokerRegistryEntry`] that describes its config
//! fields, allowing the setup wizard to render forms without hard-coded knowledge
//! of individual brokers.

use std::collections::HashMap;

use bon::Builder;
use serde::{Deserialize, Serialize};
use snafu::Snafu;

use crate::account_config::BrokerConfig;
use crate::broker::Broker;

/// The type of value a configuration field accepts.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, strum::Display, strum::EnumString,
)]
#[strum(serialize_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum ConfigFieldType {
    /// Free-form text input.
    Text,
    /// Masked text input for secrets.
    Password,
    /// Numeric input.
    Number,
    /// True/false toggle.
    Boolean,
    /// Pick from a predefined list of options.
    Select,
}

/// A selectable option for [`ConfigFieldType::Select`] fields.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SelectOption {
    /// The stored value when this option is chosen.
    pub value: String,
    /// The human-readable label shown in the UI.
    pub label: String,
}

/// Describes a single configuration field for a broker.
#[derive(Debug, Clone, Serialize, Deserialize, Builder)]
pub struct ConfigField {
    /// Machine-readable field name (used as the key in config maps).
    pub name: String,
    /// The input type for this field.
    pub field_type: ConfigFieldType,
    /// Human-readable label shown in the UI.
    pub label: String,
    /// Placeholder text shown when the field is empty.
    pub placeholder: Option<String>,
    /// Default value if the user provides none.
    pub default: Option<String>,
    /// Whether the field must be filled.
    pub required: bool,
    /// Available choices for [`ConfigFieldType::Select`] fields.
    pub options: Vec<SelectOption>,
    /// Help text describing the field's purpose.
    pub description: Option<String>,
    /// Whether the field contains sensitive data (passwords, keys).
    pub sensitive: bool,
}

/// Typed broker config value produced by a registry entry's `create_config` fn.
pub enum BrokerConfigValue {
    /// Paper broker configuration.
    Paper(crate::account_config::PaperBrokerConfig),
    /// CCXT broker configuration.
    Ccxt(crate::account_config::CcxtBrokerConfig),
}

impl BrokerConfigValue {
    /// Convert into the unified [`BrokerConfig`] enum used by account config.
    pub fn into_broker_config(self) -> BrokerConfig {
        match self {
            Self::Paper(cfg) => BrokerConfig::Paper(cfg),
            Self::Ccxt(cfg) => BrokerConfig::Ccxt(cfg),
        }
    }
}

/// Errors that can occur during broker registry operations.
#[derive(Debug, Snafu)]
#[snafu(visibility(pub))]
pub enum BrokerRegistryError {
    /// A required configuration field was not provided.
    #[snafu(display("missing required field: {field}"))]
    MissingField {
        /// Name of the missing field.
        field: String,
    },
    /// A configuration field has an invalid value.
    #[snafu(display("invalid value for field '{field}': {reason}"))]
    InvalidValue {
        /// Name of the field with the invalid value.
        field: String,
        /// Why the value is invalid.
        reason: String,
    },
    /// The requested broker type key is not registered.
    #[snafu(display("unknown broker type: {type_key}"))]
    UnknownType {
        /// The unrecognized type key.
        type_key: String,
    },
}

/// Result type for broker registry operations.
pub type Result<T> = std::result::Result<T, BrokerRegistryError>;

/// A self-describing broker registration entry.
///
/// Each broker provides one of these to declare its identity, config schema,
/// and factory functions. The setup wizard uses this metadata to dynamically
/// render broker configuration forms.
pub struct BrokerRegistryEntry {
    /// Unique identifier for this broker type (e.g. "paper", "ccxt").
    pub type_key: &'static str,
    /// Human-readable broker name.
    pub name: &'static str,
    /// Short description of the broker.
    pub description: &'static str,
    /// Returns the config fields this broker requires.
    pub config_fields: fn() -> Vec<ConfigField>,
    /// Create a [`Broker`] instance from a raw config map.
    pub create_broker:
        fn(&HashMap<String, String>) -> std::result::Result<Box<dyn Broker>, BrokerRegistryError>,
    /// Create a typed [`BrokerConfigValue`] from a raw config map.
    pub create_config:
        fn(&HashMap<String, String>) -> std::result::Result<BrokerConfigValue, BrokerRegistryError>,
}

// BROKER_REGISTRY and find_broker() are added in Task 2 once register_all() exists.
