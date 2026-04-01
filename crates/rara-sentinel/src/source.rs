//! Data source trait and raw signal type for sentinel ingestion.

use async_trait::async_trait;
use snafu::Snafu;

/// A raw, unprocessed signal from an external data source.
#[derive(Debug, Clone)]
pub struct RawSignal {
    /// Name of the source that produced this signal.
    pub source_name: String,
    /// Raw textual content to be analyzed.
    pub content:     String,
    /// Arbitrary metadata from the source.
    pub metadata:    serde_json::Value,
    /// When the raw signal was captured.
    pub timestamp:   jiff::Timestamp,
}

/// Errors that can occur when polling a data source.
#[derive(Debug, Snafu)]
#[snafu(visibility(pub))]
pub enum SourceError {
    /// A fetch operation failed.
    #[snafu(display("fetch error: {message}"))]
    Fetch {
        /// Description of the failure.
        message: String,
    },
}

/// Trait for external data sources that produce raw signals.
#[async_trait]
pub trait DataSource: Send + Sync {
    /// Returns the human-readable name of this data source.
    fn name(&self) -> &str;

    /// Poll the data source for new raw signals.
    async fn poll(&self) -> Result<Vec<RawSignal>, SourceError>;
}
