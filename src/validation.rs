//! Startup configuration validation.
//!
//! Checks database connectivity and LLM backend availability at startup,
//! returning actionable error messages with fix suggestions.

use snafu::{ResultExt, Snafu};

/// Errors from startup validation.
#[derive(Debug, Snafu)]
#[snafu(visibility(pub))]
pub enum ValidationError {
    /// Database connection failed.
    #[snafu(display(
        "database connection failed: {source}\n\nFix: check that PostgreSQL/TimescaleDB is \
         running and the URL is correct.\n  Current URL: {url}\n  Override with: \
         RARA_DB_URL=postgres://user:pass@host:5432/db"
    ))]
    DatabaseConnection { url: String, source: sqlx::Error },

    /// LLM backend not available in PATH.
    #[snafu(display(
        "LLM backend '{backend}' is not available\n\nFix: ensure the agent backend is installed \
         and accessible in PATH.\n  Override with: RARA_LLM_BACKEND=claude"
    ))]
    LlmBackendUnavailable { backend: String },
}

/// Result type for validation operations.
pub type Result<T> = std::result::Result<T, ValidationError>;

/// Validate database connectivity by attempting a short-lived connection.
pub async fn validate_database(url: &str) -> Result<()> {
    use sqlx::postgres::PgPoolOptions;

    PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(std::time::Duration::from_secs(5))
        .connect(url)
        .await
        .context(DatabaseConnectionSnafu { url })?;

    Ok(())
}

/// Validate LLM backend availability by checking whether the command exists in
/// PATH.
pub fn validate_llm_backend(backend: &str) -> Result<()> {
    match which::which(backend) {
        Ok(_) => Ok(()),
        Err(_) => LlmBackendUnavailableSnafu { backend }.fail(),
    }
}

/// Run all startup validations and collect errors.
///
/// Returns a list of validation errors. An empty list means all checks passed.
pub async fn validate_startup(cfg: &crate::app_config::AppConfig) -> Vec<ValidationError> {
    let mut errors = Vec::new();

    if let Err(e) = validate_database(&cfg.database.url).await {
        errors.push(e);
    }

    if let Err(e) = validate_llm_backend(&cfg.agent.backend) {
        errors.push(e);
    }

    errors
}
