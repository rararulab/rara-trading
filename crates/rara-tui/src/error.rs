//! Error types for the TUI crate.

use snafu::Snafu;

/// Errors that can occur in the TUI application.
#[derive(Debug, Snafu)]
#[snafu(visibility(pub))]
pub enum TuiError {
    /// Terminal I/O failed.
    #[snafu(display("terminal I/O error: {source}"))]
    Io { source: std::io::Error },

    /// gRPC connection or call failed.
    #[snafu(display("gRPC error: {source}"))]
    Grpc { source: tonic::transport::Error },

    /// gRPC call returned an error status.
    #[snafu(display("gRPC status: {source}"))]
    GrpcStatus { source: tonic::Status },
}

/// Convenience alias for TUI results.
pub type Result<T> = std::result::Result<T, TuiError>;
