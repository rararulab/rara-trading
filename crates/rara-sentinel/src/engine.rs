//! Sentinel engine that orchestrates data source polling, LLM analysis,
//! and event publishing.

use std::sync::Arc;

use rara_domain::{
    event::{Event, EventType},
    sentinel::SentinelSignal,
};
use rara_event_bus::bus::EventBus;
use rara_infra::llm::LlmClient;
use snafu::{ResultExt, Snafu};

use crate::{
    analyzer::{AnalyzerError, SignalAnalyzer},
    source::{DataSource, SourceError},
};

/// Errors that can occur in the sentinel engine.
#[derive(Debug, Snafu)]
#[snafu(visibility(pub))]
pub enum SentinelError {
    /// A data source failed to poll.
    #[snafu(display("source error in {source_name}: {source}"))]
    Source {
        /// Name of the failing source.
        source_name: String,
        /// The underlying source error.
        source:      SourceError,
    },
    /// The analyzer failed to classify a signal.
    #[snafu(display("analyzer error: {source}"))]
    Analyzer {
        /// The underlying analyzer error.
        source: AnalyzerError,
    },
    /// Failed to publish an event to the event bus.
    #[snafu(display("event bus error: {source}"))]
    EventBus {
        /// The underlying store error.
        source: rara_event_bus::store::StoreError,
    },
}

/// Orchestrates sentinel surveillance: polls data sources, analyzes signals
/// with an LLM, and publishes actionable events to the event bus.
pub struct SentinelEngine<L: LlmClient> {
    /// Registered data sources to poll.
    sources:   Vec<Box<dyn DataSource>>,
    /// LLM-backed signal analyzer.
    analyzer:  SignalAnalyzer<L>,
    /// Event bus for publishing detected signals.
    event_bus: Arc<EventBus>,
}

impl<L: LlmClient> SentinelEngine<L> {
    /// Create a new sentinel engine.
    pub fn new(
        sources: Vec<Box<dyn DataSource>>,
        analyzer: SignalAnalyzer<L>,
        event_bus: Arc<EventBus>,
    ) -> Self {
        Self {
            sources,
            analyzer,
            event_bus,
        }
    }

    /// Poll all data sources, analyze raw signals, publish actionable events,
    /// and return the list of detected signals.
    pub async fn poll_and_analyze(&self) -> Result<Vec<SentinelSignal>, SentinelError> {
        let mut all_raw = Vec::new();

        for source in &self.sources {
            let raw_signals = source.poll().await.context(SourceSnafu {
                source_name: source.name(),
            })?;
            all_raw.extend(raw_signals);
        }

        let mut detected = Vec::new();

        for raw in &all_raw {
            let maybe_signal = self.analyzer.analyze(raw).await.context(AnalyzerSnafu)?;

            if let Some(signal) = maybe_signal {
                let event = Event::builder()
                    .event_type(EventType::SentinelSignalDetected)
                    .source("sentinel-engine")
                    .correlation_id(signal.id.to_string())
                    .payload(serde_json::to_value(&signal).expect("signal must serialize"))
                    .build();

                self.event_bus.publish(&event).context(EventBusSnafu)?;
                detected.push(signal);
            }
        }

        Ok(detected)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use rara_agent::{
        backend::{CliBackend, OutputFormat, PromptMode},
        executor::CliExecutor,
    };
    use rara_domain::sentinel::{SignalSource, SignalType};
    use serde_json::json;

    use super::*;
    use crate::{source::RawSignal, sources::webhook::WebhookDataSource};

    fn echo_executor(response: &str) -> CliExecutor {
        CliExecutor::new(CliBackend {
            command:       "sh".to_string(),
            args:          vec!["-c".to_string(), format!("printf '{response}\\n'")],
            prompt_mode:   PromptMode::Arg,
            prompt_flag:   None,
            output_format: OutputFormat::Text,
            env_vars:      vec![],
        })
    }

    #[tokio::test]
    async fn poll_and_analyze_publishes_critical_signals() {
        let dir = tempfile::tempdir().unwrap();
        let event_bus = Arc::new(EventBus::open(dir.path()).unwrap());

        let raw = RawSignal {
            source_name: "webhook-news".to_owned(),
            content:     "Exchange hacked, funds drained".to_owned(),
            metadata:    json!({}),
            timestamp:   jiff::Timestamp::now(),
        };

        let source = WebhookDataSource::new("webhook-news");
        source.push(raw).await;

        let executor = echo_executor(
            "SEVERITY: Critical\nTYPE: BlackSwan\nCONTRACTS: BTC-PERP\nSUMMARY: Exchange hack \
             detected",
        );

        let analyzer = SignalAnalyzer::new(executor);
        let engine = SentinelEngine::new(vec![Box::new(source)], analyzer, event_bus.clone());

        let signals = engine.poll_and_analyze().await.unwrap();
        assert_eq!(signals.len(), 1);
        assert!(signals[0].should_block_trading());

        // Verify event was published to the bus
        let events = event_bus.store().read_topic("sentinel", 0, 10).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, EventType::SentinelSignalDetected);
    }

    #[tokio::test]
    async fn poll_and_analyze_skips_none_severity() {
        let dir = tempfile::tempdir().unwrap();
        let event_bus = Arc::new(EventBus::open(dir.path()).unwrap());

        let raw = RawSignal {
            source_name: "webhook-news".to_owned(),
            content:     "Routine market update".to_owned(),
            metadata:    json!({}),
            timestamp:   jiff::Timestamp::now(),
        };

        let source = WebhookDataSource::new("webhook-news");
        source.push(raw).await;

        let executor = echo_executor(
            "SEVERITY: None\nTYPE: SentimentShift\nCONTRACTS: \nSUMMARY: No actionable signal",
        );

        let analyzer = SignalAnalyzer::new(executor);
        let engine = SentinelEngine::new(vec![Box::new(source)], analyzer, event_bus.clone());

        let signals = engine.poll_and_analyze().await.unwrap();
        assert!(signals.is_empty());

        // No events should be published
        let events = event_bus.store().read_topic("sentinel", 0, 10).unwrap();
        assert!(events.is_empty());
    }

    #[tokio::test]
    async fn trump_code_signal_flows_through_engine() {
        let dir = tempfile::tempdir().unwrap();
        let event_bus = Arc::new(EventBus::open(dir.path()).unwrap());

        // Simulate a trump-code signal via webhook
        let raw = RawSignal {
            source_name: "trump-code".to_owned(),
            content:     "Trump Code signals for 2026-03-25: TARIFF(x3), DEAL(x1). Consensus: \
                          BULLISH. Posts today: 12. Opus insight: Deal signals suggest resolution"
                .to_owned(),
            metadata:    json!({
                "consensus": "BULLISH",
                "signal_confidence": {"TARIFF": 0.65, "DEAL": 0.72},
            }),
            timestamp:   jiff::Timestamp::now(),
        };

        let source = WebhookDataSource::new("trump-code");
        source.push(raw).await;

        let executor = echo_executor(
            "SEVERITY: Warning\nTYPE: PoliticalSignal\nCONTRACTS: SPY,SPX\nSUMMARY: Trump tariff \
             signals with deal offset",
        );

        let analyzer = SignalAnalyzer::new(executor);
        let engine = SentinelEngine::new(vec![Box::new(source)], analyzer, event_bus.clone());

        let signals = engine.poll_and_analyze().await.unwrap();
        assert_eq!(signals.len(), 1);
        assert_eq!(signals[0].signal_type, SignalType::PoliticalSignal);
        assert_eq!(signals[0].source, SignalSource::TrumpCode);
        assert!(!signals[0].should_block_trading()); // Warning, not Critical
    }
}
