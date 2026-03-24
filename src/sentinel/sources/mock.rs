//! Mock data source for testing the sentinel pipeline.

use std::sync::Mutex;

use async_trait::async_trait;

use crate::sentinel::source::{DataSource, RawSignal, SourceError};

/// A mock data source that returns pre-configured raw signals.
pub struct MockDataSource {
    /// Human-readable name for this mock source.
    name: String,
    /// Queue of signals to return on each poll call.
    signals: Mutex<Vec<RawSignal>>,
}

impl MockDataSource {
    /// Create a new mock data source with the given name and signal queue.
    pub fn new(name: impl Into<String>, signals: Vec<RawSignal>) -> Self {
        Self {
            name: name.into(),
            signals: Mutex::new(signals),
        }
    }
}

#[async_trait]
impl DataSource for MockDataSource {
    fn name(&self) -> &str {
        &self.name
    }

    async fn poll(&self) -> Result<Vec<RawSignal>, SourceError> {
        let drained: Vec<RawSignal> =
            self.signals.lock().expect("mock lock poisoned").drain(..).collect();
        Ok(drained)
    }
}
