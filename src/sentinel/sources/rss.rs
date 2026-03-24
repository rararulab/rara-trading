//! RSS/Atom feed data source for sentinel ingestion.

use async_trait::async_trait;
use bon::Builder;

use crate::sentinel::source::{DataSource, RawSignal, SourceError};

/// Data source that polls an RSS or Atom feed for news signals.
#[derive(Builder)]
pub struct RssDataSource {
    /// Human-readable name for this source.
    name: String,
    /// URL of the RSS/Atom feed.
    url: String,
    /// HTTP client for fetching feeds.
    client: reqwest::Client,
}

#[async_trait]
impl DataSource for RssDataSource {
    fn name(&self) -> &str {
        &self.name
    }

    async fn poll(&self) -> Result<Vec<RawSignal>, SourceError> {
        let response = self
            .client
            .get(&self.url)
            .send()
            .await
            .map_err(|e| SourceError::Fetch {
                message: format!("HTTP request to {} failed: {e}", self.url),
            })?;

        let bytes = response.bytes().await.map_err(|e| SourceError::Fetch {
            message: format!("reading body from {} failed: {e}", self.url),
        })?;

        let feed =
            feed_rs::parser::parse(&bytes[..]).map_err(|e| SourceError::Fetch {
                message: format!("failed to parse feed from {}: {e}", self.url),
            })?;

        let signals = feed
            .entries
            .into_iter()
            .map(|entry| {
                let title = entry
                    .title
                    .as_ref()
                    .map_or_else(String::new, |t| t.content.clone());

                let description = entry
                    .summary
                    .as_ref()
                    .map_or_else(String::new, |s| s.content.clone());

                let content = if description.is_empty() {
                    title.clone()
                } else {
                    format!("{title}\n\n{description}")
                };

                let link = entry
                    .links
                    .first()
                    .map(|l| l.href.clone())
                    .unwrap_or_default();

                let published = entry
                    .published
                    .or(entry.updated)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default();

                let metadata = serde_json::json!({
                    "title": title,
                    "link": link,
                    "published": published,
                });

                RawSignal {
                    source_name: self.name.clone(),
                    content,
                    metadata,
                    timestamp: jiff::Timestamp::now(),
                }
            })
            .collect();

        Ok(signals)
    }
}
