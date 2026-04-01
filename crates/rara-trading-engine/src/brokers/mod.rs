//! Broker implementations.

use crate::broker_registry::BrokerRegistryEntry;

pub mod ccxt;
pub mod paper;

/// Collect all broker registry entries.
///
/// Only real exchange brokers are registered. The `paper` module provides a
/// test-only broker that is not user-configurable.
pub fn register_all() -> Vec<BrokerRegistryEntry> {
    vec![ccxt::registry_entry()]
}
