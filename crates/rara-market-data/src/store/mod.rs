//! `TimescaleDB`-backed market data storage.

pub mod candle;
pub mod tick;

use snafu::{ResultExt, Snafu};
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;

/// Errors from market data store operations.
#[derive(Debug, Snafu)]
#[snafu(visibility(pub))]
pub enum StoreError {
    /// Database connection or query failed.
    #[snafu(display("database error: {source}"))]
    Database { source: sqlx::Error },

    /// Database migration failed.
    #[snafu(display("migration error: {source}"))]
    Migration { source: sqlx::migrate::MigrateError },
}

/// Convenience alias.
pub type Result<T> = std::result::Result<T, StoreError>;

/// `TimescaleDB` connection pool wrapper for all market data operations.
pub struct MarketStore {
    /// Underlying connection pool.
    pub pool: PgPool,
}

impl MarketStore {
    /// Connect to the database using the given URL.
    pub async fn connect(database_url: &str) -> Result<Self> {
        let pool = PgPoolOptions::new()
            .max_connections(5)
            .connect(database_url)
            .await
            .context(DatabaseSnafu)?;
        Ok(Self { pool })
    }

    /// Run embedded sqlx migrations to create/update tables.
    pub async fn migrate(&self) -> Result<()> {
        sqlx::migrate!("./migrations")
            .run(&self.pool)
            .await
            .context(MigrationSnafu)
    }
}
