//! Manages a child gRPC server process for standalone TUI mode.
//!
//! When the TUI is launched without an explicit `--server` address, this module
//! spawns the same binary with `serve --port <port>` and ensures cleanup on exit.

use std::net::TcpListener;
use std::process::Stdio;

use snafu::ResultExt;
use tokio::process::{Child, Command};
use tracing::{info, warn};

use crate::error::{IoSnafu, Result};

/// Guard that owns a spawned gRPC server child process.
///
/// When dropped, it attempts to kill the child process to prevent orphans.
/// The `port` field records which port the server is listening on.
pub struct ServerProcess {
    pub port: u16,
    child: Child,
}

impl ServerProcess {
    /// Spawn a `rara-trading serve --port <port>` child process.
    ///
    /// Uses `std::env::current_exe()` to locate the binary, picks an available
    /// port via ephemeral binding, and redirects stdout/stderr to `/dev/null`.
    pub async fn spawn() -> Result<Self> {
        let port = pick_available_port()?;
        let exe = std::env::current_exe().context(IoSnafu)?;

        info!(port, ?exe, "spawning gRPC server subprocess");

        let child = Command::new(exe)
            .args(["serve", "--port", &port.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .context(IoSnafu)?;

        let process = Self { port, child };

        // Give the server a moment to bind the port
        process.wait_for_ready().await;

        Ok(process)
    }

    /// Build the gRPC server address string for this child process.
    pub fn server_addr(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    /// Gracefully shut down the child process.
    ///
    /// Sends SIGKILL via `tokio::process::Child::kill` and waits for exit.
    /// Errors are logged but not propagated since this runs during cleanup.
    pub async fn shutdown(&mut self) {
        info!(port = self.port, "shutting down gRPC server subprocess");
        if let Err(e) = self.child.kill().await {
            warn!("failed to kill server subprocess: {e}");
        }
    }

    /// Poll the port until it accepts a TCP connection, with a timeout.
    async fn wait_for_ready(&self) {
        let addr = format!("127.0.0.1:{}", self.port);
        for _ in 0..20 {
            if std::net::TcpStream::connect(&addr).is_ok() {
                info!(port = self.port, "gRPC server subprocess is ready");
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        warn!(
            port = self.port,
            "gRPC server subprocess did not become ready within 2s, proceeding anyway"
        );
    }
}

/// Find an available TCP port by binding to port 0 and reading the assigned port.
fn pick_available_port() -> Result<u16> {
    let listener = TcpListener::bind("127.0.0.1:0").context(IoSnafu)?;
    let port = listener.local_addr().context(IoSnafu)?.port();
    Ok(port)
}
