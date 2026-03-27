//! Lightweight health probes for external service dependencies.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tokio::net::TcpStream;
use tracing::debug;

/// Configuration for health probes.
pub struct HealthConfig {
    /// `PostgreSQL` connection URL (parsed for host:port TCP probe).
    pub database_url: String,
    /// Agent backend command name to look up in PATH.
    pub llm_backend: String,
    /// Shared flag set by the WebSocket layer when a connection is active.
    pub ws_connected: Arc<AtomicBool>,
}

/// Probe results for a single status poll.
pub struct HealthStatus {
    /// Whether the database TCP port is reachable.
    pub database_connected: bool,
    /// Whether the WebSocket market-data feed is active.
    pub websocket_connected: bool,
    /// Whether the configured LLM backend CLI is found in PATH.
    pub llm_available: bool,
}

/// Run all health probes and return aggregated status.
pub async fn probe(config: &HealthConfig) -> HealthStatus {
    let database_connected = probe_database(&config.database_url).await;
    let llm_available = probe_llm(&config.llm_backend);
    let websocket_connected = config.ws_connected.load(Ordering::Relaxed);

    HealthStatus {
        database_connected,
        websocket_connected,
        llm_available,
    }
}

/// TCP connect probe to the `PostgreSQL` host:port extracted from the URL.
async fn probe_database(url: &str) -> bool {
    let Some(addr) = parse_pg_host_port(url) else {
        debug!("failed to parse database URL for health probe");
        return false;
    };

    tokio::time::timeout(
        std::time::Duration::from_millis(500),
        TcpStream::connect(&addr),
    )
    .await
    .is_ok_and(|r| r.is_ok())
}

/// Check if the LLM backend command exists in PATH.
fn probe_llm(backend: &str) -> bool {
    which::which(backend).is_ok()
}

/// Extract `host:port` from a `PostgreSQL` URL.
///
/// Handles both `postgres://user:pass@host:port/db` and
/// `postgres://user:pass@host/db` (defaults to port 5432).
fn parse_pg_host_port(url: &str) -> Option<String> {
    // Strip scheme
    let after_scheme = url.split("://").nth(1)?;
    // Strip userinfo
    let after_at = after_scheme.rsplit('@').next()?;
    // Strip database path and query
    let host_port = after_at.split('/').next()?;

    if host_port.contains(':') {
        Some(host_port.to_string())
    } else {
        Some(format!("{host_port}:5432"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_pg_url_with_port() {
        let result = parse_pg_host_port("postgres://user:pass@db.example.com:5433/mydb");
        assert_eq!(result.as_deref(), Some("db.example.com:5433"));
    }

    #[test]
    fn parse_pg_url_without_port() {
        let result = parse_pg_host_port("postgres://user:pass@localhost/mydb");
        assert_eq!(result.as_deref(), Some("localhost:5432"));
    }

    #[test]
    fn parse_pg_url_invalid() {
        assert!(parse_pg_host_port("not-a-url").is_none());
    }
}
