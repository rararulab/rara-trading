//! Sentinel signal types for market surveillance and risk gating.

use bon::Builder;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Classification of detected market signals.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SignalType {
    /// Regulatory enforcement or policy change.
    RegulatoryAction,
    /// Extreme market event.
    BlackSwan,
    /// Unusual volume spike.
    AbnormalVolatility,
    /// Shift in market sentiment.
    SentimentShift,
    /// Anomalous on-chain activity.
    OnChainAnomaly,
}

/// Where the signal was detected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SignalSource {
    /// RSS news feeds.
    NewsRss,
    /// Social media platforms.
    SocialMedia,
    /// Blockchain data.
    OnChain,
    /// Price/volume action.
    PriceAction,
}

/// Severity level of a sentinel signal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Severity {
    /// Informational — no action needed.
    Info,
    /// Warning — review recommended.
    Warning,
    /// Critical — may require halting trading.
    Critical,
}

/// A signal detected by the sentinel surveillance system.
#[derive(Debug, Clone, Serialize, Deserialize, Builder)]
pub struct SentinelSignal {
    #[builder(default = Uuid::new_v4())]
    id: Uuid,
    signal_type: SignalType,
    severity: Severity,
    source: SignalSource,
    affected_contracts: Vec<String>,
    #[builder(into)]
    summary: String,
    raw_data: serde_json::Value,
    #[builder(default = jiff::Timestamp::now())]
    detected_at: jiff::Timestamp,
}

impl SentinelSignal {
    /// Returns the signal identifier.
    pub const fn id(&self) -> Uuid {
        self.id
    }

    /// Returns the severity level.
    pub const fn severity(&self) -> Severity {
        self.severity
    }

    /// Returns the list of affected contract IDs.
    pub fn affected_contracts(&self) -> &[String] {
        &self.affected_contracts
    }

    /// Returns `true` if this signal is critical and should block trading.
    pub fn should_block_trading(&self) -> bool {
        self.severity == Severity::Critical
    }
}
