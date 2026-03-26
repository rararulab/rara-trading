//! Centralized structured logging initialization.
//!
//! Supports dual-output logging: pretty-printed to stderr for development,
//! and JSON-formatted to a log file for production analysis.

use std::path::Path;

use serde::{Deserialize, Serialize};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer};

/// Logging configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LoggingConfig {
    /// Default log level (trace, debug, info, warn, error).
    pub level: String,
    /// Path to log file directory. If set, JSON logs are written here.
    pub log_dir: Option<String>,
    /// Log format for stderr: "pretty" or "json".
    pub stderr_format: String,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: "info".to_string(),
            log_dir: None,
            stderr_format: "pretty".to_string(),
        }
    }
}

/// Initialize the global tracing subscriber based on the given configuration.
///
/// Returns a [`WorkerGuard`] when file logging is enabled. The caller **must**
/// hold this guard for the lifetime of the program to ensure buffered log
/// entries are flushed to disk on shutdown.
///
/// # Layers
///
/// - **stderr layer**: always active; uses pretty or JSON format based on
///   `config.stderr_format`.
/// - **file layer**: only active when `config.log_dir` is set; writes
///   JSON-formatted logs via a non-blocking appender.
///
/// The log level is controlled by the `RUST_LOG` environment variable,
/// falling back to `config.level` if `RUST_LOG` is not set.
pub fn init_logging(config: &LoggingConfig) -> Option<WorkerGuard> {
    let env_filter = build_env_filter(&config.level);

    match (&config.log_dir, config.stderr_format.as_str()) {
        // File logging enabled + pretty stderr
        (Some(dir), "pretty") => {
            let (file_layer, guard) = build_file_layer(dir);
            tracing_subscriber::registry()
                .with(env_filter)
                .with(build_pretty_stderr_layer())
                .with(file_layer)
                .init();
            Some(guard)
        }
        // File logging enabled + JSON stderr
        (Some(dir), _) => {
            let (file_layer, guard) = build_file_layer(dir);
            tracing_subscriber::registry()
                .with(env_filter)
                .with(build_json_stderr_layer())
                .with(file_layer)
                .init();
            Some(guard)
        }
        // No file logging + pretty stderr
        (None, "pretty") => {
            tracing_subscriber::registry()
                .with(env_filter)
                .with(build_pretty_stderr_layer())
                .init();
            None
        }
        // No file logging + JSON stderr
        (None, _) => {
            tracing_subscriber::registry()
                .with(env_filter)
                .with(build_json_stderr_layer())
                .init();
            None
        }
    }
}

/// Build an `EnvFilter` from `RUST_LOG`, falling back to the configured level.
fn build_env_filter(default_level: &str) -> EnvFilter {
    EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        EnvFilter::new(default_level)
    })
}

/// Build a pretty-formatted stderr layer.
fn build_pretty_stderr_layer<S>() -> impl Layer<S>
where
    S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    tracing_subscriber::fmt::layer()
        .with_writer(std::io::stderr)
        .with_target(true)
        .with_thread_ids(false)
        .with_file(false)
        .with_line_number(false)
}

/// Build a JSON-formatted stderr layer.
fn build_json_stderr_layer<S>() -> impl Layer<S>
where
    S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    tracing_subscriber::fmt::layer()
        .json()
        .with_writer(std::io::stderr)
        .with_target(true)
}

/// Build a JSON-formatted file layer with non-blocking writes.
///
/// Creates the log directory if it does not exist. Log files are
/// automatically rotated daily with the prefix `rara-trading`.
fn build_file_layer<S>(
    dir: &str,
) -> (impl Layer<S>, WorkerGuard)
where
    S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    let path = Path::new(dir);
    // Ensure the directory exists before the appender tries to write
    if let Err(e) = std::fs::create_dir_all(path) {
        eprintln!("warning: failed to create log directory {dir}: {e}");
    }

    let file_appender = tracing_appender::rolling::daily(path, "rara-trading.log");
    let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);

    let layer = tracing_subscriber::fmt::layer()
        .json()
        .with_writer(non_blocking)
        .with_target(true)
        .with_ansi(false);

    (layer, guard)
}
