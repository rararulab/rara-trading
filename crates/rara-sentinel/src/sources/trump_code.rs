//! Trump-code political signal data source for sentinel ingestion.

use std::collections::HashMap;

use async_trait::async_trait;
use bon::Builder;
use serde::Deserialize;

use crate::source::{DataSource, RawSignal, SourceError};

/// API response from the trump-code `/api/signals` endpoint.
#[derive(Debug, Deserialize)]
pub struct SignalsResponse {
    /// Date string for the current analysis window.
    pub date: String,
    /// Active signal types detected today.
    pub signals: Vec<String>,
    /// Number of posts analyzed.
    pub posts: u32,
    /// Overall market consensus derived from signals.
    pub consensus: String,
    /// Per-day signal breakdown keyed by date string.
    pub recent_days: HashMap<String, Vec<DaySignal>>,
    /// Confidence scores per signal type (0.0–1.0).
    pub signal_confidence: HashMap<String, f64>,
    /// Playbook summary with notable patterns.
    pub playbook_summary: PlaybookSummary,
    /// Optional long-form insight from Opus analysis.
    #[serde(default)]
    pub opus_insight: String,
}

/// A single signal type and its occurrence count for a given day.
#[derive(Debug, Deserialize)]
pub struct DaySignal {
    /// Signal type name (e.g. "TARIFF", "DEAL", "RELIEF").
    #[serde(rename = "type")]
    pub signal_type: String,
    /// How many times this signal was detected.
    pub count: u32,
}

/// Notable pattern summaries from the playbook analysis.
#[derive(Debug, Deserialize)]
pub struct PlaybookSummary {
    /// The most dangerous signal pattern observed.
    pub most_dangerous: String,
    /// The most profitable signal pattern observed.
    pub most_profitable: String,
    /// The biggest surprise in the data.
    pub biggest_surprise: String,
}

impl SignalsResponse {
    /// Converts the API response into a vector of [`RawSignal`]s.
    ///
    /// Produces one `RawSignal` per day in `recent_days` that contains at
    /// least one signal. Results are sorted by date descending (most recent
    /// first).
    pub fn into_raw_signals(self) -> Vec<RawSignal> {
        let mut days: Vec<(String, Vec<DaySignal>)> = self
            .recent_days
            .into_iter()
            .filter(|(_, signals)| !signals.is_empty())
            .collect();

        // Most recent date first
        days.sort_by(|a, b| b.0.cmp(&a.0));

        let now = jiff::Timestamp::now();

        // Extract shared fields before the iterator borrows them
        let consensus = self.consensus;
        let opus_insight = self.opus_insight;
        let posts = self.posts;
        let signal_confidence = self.signal_confidence;
        let playbook = self.playbook_summary;

        days.into_iter()
            .map(|(date, day_signals)| {
                let signal_types: Vec<String> = day_signals
                    .iter()
                    .map(|s| format!("{}(x{})", s.signal_type, s.count))
                    .collect();

                let content = format!(
                    "Date: {date} | Signals: {} | Consensus: {consensus} | Opus: {opus_insight}",
                    signal_types.join(", "),
                );

                let day_details: Vec<serde_json::Value> = day_signals
                    .iter()
                    .map(|s| {
                        serde_json::json!({
                            "type": s.signal_type,
                            "count": s.count,
                        })
                    })
                    .collect();

                let metadata = serde_json::json!({
                    "date": date,
                    "consensus": consensus,
                    "posts_today": posts,
                    "signal_confidence": signal_confidence,
                    "playbook": {
                        "most_dangerous": playbook.most_dangerous,
                        "most_profitable": playbook.most_profitable,
                        "biggest_surprise": playbook.biggest_surprise,
                    },
                    "opus_insight": opus_insight,
                    "day_signals": day_details,
                });

                RawSignal {
                    source_name: "trump-code".to_owned(),
                    content,
                    metadata,
                    timestamp: now,
                }
            })
            .collect()
    }
}

/// Data source that polls the trump-code API for political trading signals.
#[derive(Builder)]
pub struct TrumpCodeDataSource {
    /// Base URL of the trump-code service.
    #[builder(default = "https://trumpcode.washinmura.jp".to_owned())]
    pub base_url: String,
    /// HTTP client for API requests.
    pub client: reqwest::Client,
}

#[async_trait]
impl DataSource for TrumpCodeDataSource {
    #[allow(clippy::unnecessary_literal_bound)]
    fn name(&self) -> &str {
        "trump-code"
    }

    async fn poll(&self) -> Result<Vec<RawSignal>, SourceError> {
        let url = format!("{}/api/signals", self.base_url);

        let response: SignalsResponse = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| SourceError::Fetch {
                message: format!("HTTP request to {url} failed: {e}"),
            })?
            .json()
            .await
            .map_err(|e| SourceError::Fetch {
                message: format!("failed to deserialize response from {url}: {e}"),
            })?;

        Ok(response.into_raw_signals())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_signals_response_into_raw_signals() {
        let json = serde_json::json!({
            "date": "2026-03-25",
            "signals": ["TARIFF", "DEAL"],
            "posts": 12,
            "consensus": "BULLISH",
            "recent_days": {
                "2026-03-25": [
                    {"type": "TARIFF", "count": 3},
                    {"type": "DEAL", "count": 1}
                ],
                "2026-03-24": [
                    {"type": "RELIEF", "count": 2}
                ]
            },
            "signal_confidence": {"TARIFF": 0.65, "DEAL": 0.72},
            "playbook_summary": {
                "most_dangerous": "Pure tariff day without deal signals",
                "most_profitable": "Pre-market RELIEF with low post volume",
                "biggest_surprise": "Silence days are 80% bullish"
            },
            "opus_insight": "Market expects tariff escalation but deal signals suggest resolution"
        });

        let response: SignalsResponse = serde_json::from_value(json).unwrap();
        let signals = response.into_raw_signals();

        assert_eq!(signals.len(), 2);
        assert_eq!(signals[0].source_name, "trump-code");
        assert!(signals[0].content.contains("TARIFF"));
        assert!(signals[0].content.contains("BULLISH"));
        assert_eq!(signals[0].metadata["date"], "2026-03-25");
    }

    #[test]
    fn empty_signals_response_produces_no_raw_signals() {
        let json = serde_json::json!({
            "date": "2026-03-25",
            "signals": [],
            "posts": 0,
            "consensus": "?",
            "recent_days": {},
            "signal_confidence": {},
            "playbook_summary": {
                "most_dangerous": "",
                "most_profitable": "",
                "biggest_surprise": ""
            },
            "opus_insight": ""
        });

        let response: SignalsResponse = serde_json::from_value(json).unwrap();
        let signals = response.into_raw_signals();
        assert!(signals.is_empty());
    }
}
