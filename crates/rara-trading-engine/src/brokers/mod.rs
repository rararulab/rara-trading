//! Broker implementations.

use crate::broker_registry::BrokerRegistryEntry;

pub mod ccxt;
pub mod paper;

/// Collect all broker registry entries.
pub fn register_all() -> Vec<BrokerRegistryEntry> {
    vec![paper::registry_entry(), ccxt::registry_entry()]
}
