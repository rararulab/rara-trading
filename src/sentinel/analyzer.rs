//! LLM-based signal analyzer that classifies raw signals into actionable
//! sentinel signals.

use snafu::{ResultExt, Snafu};

use crate::domain::sentinel::{Severity, SignalSource, SignalType, SentinelSignal};
use crate::infra::llm::LlmClient;
use crate::sentinel::source::RawSignal;

/// Errors that can occur during signal analysis.
#[derive(Debug, Snafu)]
#[snafu(visibility(pub))]
pub enum AnalyzerError {
    /// The LLM request failed.
    #[snafu(display("LLM error: {source}"))]
    Llm {
        /// The underlying LLM error.
        source: crate::infra::llm::LlmError,
    },
    /// The LLM response could not be parsed.
    #[snafu(display("parse error: {message}"))]
    Parse {
        /// Description of the parse failure.
        message: String,
    },
}

/// Analyzes raw signals using an LLM to determine severity, type, and
/// affected contracts.
pub struct SignalAnalyzer<L: LlmClient> {
    /// The LLM client used for classification.
    llm: L,
}

impl<L: LlmClient> SignalAnalyzer<L> {
    /// Create a new analyzer backed by the given LLM client.
    pub const fn new(llm: L) -> Self {
        Self { llm }
    }

    /// Analyze a raw signal and return an actionable `SentinelSignal` if the
    /// LLM determines it warrants attention, or `None` if no action is needed.
    pub async fn analyze(&self, raw: &RawSignal) -> Result<Option<SentinelSignal>, AnalyzerError> {
        let prompt = build_prompt(raw);
        let response = self.llm.complete(&prompt).await.context(LlmSnafu)?;
        parse_response(&response, raw)
    }
}

/// Build an LLM prompt from a raw signal.
fn build_prompt(raw: &RawSignal) -> String {
    format!(
        "Analyze the following market signal and classify it.\n\
         Source: {}\n\
         Timestamp: {}\n\
         Content: {}\n\
         Metadata: {}\n\n\
         Respond in exactly this format:\n\
         SEVERITY: Critical|Warning|Info|None\n\
         TYPE: BlackSwan|RegulatoryAction|AbnormalVolatility|SentimentShift|OnChainAnomaly\n\
         CONTRACTS: contract1,contract2\n\
         SUMMARY: one line summary",
        raw.source_name, raw.timestamp, raw.content, raw.metadata
    )
}

/// Parse the structured LLM response into an optional `SentinelSignal`.
fn parse_response(
    response: &str,
    raw: &RawSignal,
) -> Result<Option<SentinelSignal>, AnalyzerError> {
    let get_field = |prefix: &str| -> Result<&str, AnalyzerError> {
        response
            .lines()
            .find_map(|line| line.strip_prefix(prefix).map(str::trim))
            .ok_or_else(|| {
                ParseSnafu {
                    message: format!("missing {prefix} field in LLM response"),
                }
                .build()
            })
    };

    let severity_str = get_field("SEVERITY:")?;
    let type_str = get_field("TYPE:")?;
    let contracts_str = get_field("CONTRACTS:")?;
    let summary_str = get_field("SUMMARY:")?;

    // "None" severity means no actionable signal
    if severity_str.eq_ignore_ascii_case("None") {
        return Ok(None);
    }

    let severity = parse_severity(severity_str)?;
    let signal_type = parse_signal_type(type_str)?;
    let affected_contracts: Vec<String> = contracts_str
        .split(',')
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .collect();

    let signal = SentinelSignal::builder()
        .signal_type(signal_type)
        .severity(severity)
        .source(SignalSource::NewsRss)
        .affected_contracts(affected_contracts)
        .summary(summary_str)
        .raw_data(serde_json::json!({
            "source_name": raw.source_name,
            "content": raw.content,
            "llm_response": response,
        }))
        .build();

    Ok(Some(signal))
}

/// Parse a severity string into a `Severity` enum variant.
fn parse_severity(s: &str) -> Result<Severity, AnalyzerError> {
    match s.to_ascii_lowercase().as_str() {
        "critical" => Ok(Severity::Critical),
        "warning" => Ok(Severity::Warning),
        "info" => Ok(Severity::Info),
        other => Err(ParseSnafu {
            message: format!("unknown severity: {other}"),
        }
        .build()),
    }
}

/// Parse a signal type string into a `SignalType` enum variant.
fn parse_signal_type(s: &str) -> Result<SignalType, AnalyzerError> {
    match s {
        "BlackSwan" => Ok(SignalType::BlackSwan),
        "RegulatoryAction" => Ok(SignalType::RegulatoryAction),
        "AbnormalVolatility" => Ok(SignalType::AbnormalVolatility),
        "SentimentShift" => Ok(SignalType::SentimentShift),
        "OnChainAnomaly" => Ok(SignalType::OnChainAnomaly),
        other => Err(ParseSnafu {
            message: format!("unknown signal type: {other}"),
        }
        .build()),
    }
}

#[cfg(test)]
mod tests {
    use crate::agent::backend::{CliBackend, OutputFormat, PromptMode};
    use crate::agent::executor::CliExecutor;

    use super::*;

    fn echo_executor(response: &str) -> CliExecutor {
        CliExecutor::new(CliBackend {
            command: "sh".to_string(),
            args: vec!["-c".to_string(), format!("printf '{response}\\n'")],
            prompt_mode: PromptMode::Stdin,
            prompt_flag: None,
            output_format: OutputFormat::Text,
            env_vars: vec![],
        })
    }

    fn make_raw_signal() -> RawSignal {
        RawSignal {
            source_name: "test-source".to_owned(),
            content: "Major exchange hack detected".to_owned(),
            metadata: serde_json::json!({}),
            timestamp: jiff::Timestamp::now(),
        }
    }

    #[tokio::test]
    async fn parses_critical_signal_correctly() {
        let executor = echo_executor(
            "SEVERITY: Critical\nTYPE: BlackSwan\nCONTRACTS: BTC-PERP,ETH-PERP\nSUMMARY: Major exchange hack detected",
        );
        let analyzer = SignalAnalyzer::new(executor);
        let raw = make_raw_signal();

        let result = analyzer.analyze(&raw).await.expect("analysis should succeed");
        let signal = result.expect("should return Some for Critical severity");

        assert_eq!(signal.severity(), Severity::Critical);
        assert!(signal.should_block_trading());
        assert_eq!(signal.affected_contracts(), &["BTC-PERP", "ETH-PERP"]);
    }

    #[tokio::test]
    async fn none_severity_returns_none() {
        let executor = echo_executor(
            "SEVERITY: None\nTYPE: SentimentShift\nCONTRACTS: \nSUMMARY: No actionable signal",
        );
        let analyzer = SignalAnalyzer::new(executor);
        let raw = make_raw_signal();

        let result = analyzer.analyze(&raw).await.expect("analysis should succeed");
        assert!(result.is_none());
    }
}
