//! gRPC server for the rara-trading TUI dashboard.
//!
//! Exposes system status and event streaming over gRPC so that the TUI client
//! can render a live dashboard without direct access to internal state.

use std::sync::Arc;

use rara_event_bus::bus::EventBus;

pub mod health;
pub mod service;

/// Generated protobuf types for the rara gRPC service.
#[allow(clippy::all, clippy::pedantic, clippy::nursery)]
pub mod rara_proto {
    tonic::include_proto!("rara");
}

/// Build a fully-wired gRPC service ready to be added to a tonic server.
///
/// Combines the event bus (for `StreamEvents`) and health config (for
/// `GetSystemStatus` probes) into a single `RaraServiceServer`.
pub fn build_service(
    event_bus: Arc<EventBus>,
    health_config: health::HealthConfig,
) -> rara_proto::rara_service_server::RaraServiceServer<service::RaraServiceImpl> {
    let svc = service::RaraServiceImpl::with_event_bus(event_bus).health(health_config);
    rara_proto::rara_service_server::RaraServiceServer::new(svc)
}
