//! Webhook/push-based data source for sentinel ingestion.

use async_trait::async_trait;
use tokio::sync::Mutex;

use crate::source::{DataSource, RawSignal, SourceError};

/// Data source that receives signals pushed via a channel.
/// Useful for webhook endpoints or API-push integrations.
pub struct WebhookDataSource {
    /// Human-readable name for this source.
    name: String,
    /// Internal buffer of signals waiting to be drained.
    buffer: Mutex<Vec<RawSignal>>,
}

impl WebhookDataSource {
    /// Create a new webhook data source with the given name and an empty buffer.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            buffer: Mutex::new(Vec::new()),
        }
    }

    /// Push a raw signal into the internal buffer for the next poll cycle.
    pub async fn push(&self, signal: RawSignal) {
        self.buffer.lock().await.push(signal);
    }
}

#[async_trait]
impl DataSource for WebhookDataSource {
    fn name(&self) -> &str {
        &self.name
    }

    async fn poll(&self) -> Result<Vec<RawSignal>, SourceError> {
        let drained = self.buffer.lock().await.drain(..).collect();
        Ok(drained)
    }
}
