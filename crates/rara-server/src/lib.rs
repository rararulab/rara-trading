//! gRPC server for the rara-trading TUI dashboard.
//!
//! Exposes system status and event streaming over gRPC so that the TUI client
//! can render a live dashboard without direct access to internal state.

pub mod service;

/// Generated protobuf types for the rara gRPC service.
#[allow(clippy::all, clippy::pedantic, clippy::nursery)]
pub mod rara_proto {
    tonic::include_proto!("rara");
}
