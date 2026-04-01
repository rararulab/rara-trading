//! Strategy code generation from hypotheses using an LLM.

use std::sync::Arc;

use rara_domain::research::Hypothesis;
use rara_infra::llm::LlmClient;
use snafu::{ResultExt, Snafu};

/// Errors from strategy code generation.
#[derive(Debug, Snafu)]
#[snafu(visibility(pub))]
pub enum StrategyCoderError {
    /// LLM call failed.
    #[snafu(display("LLM error: {source}"))]
    Llm {
        /// The underlying LLM error.
        source: rara_infra::llm::LlmError,
    },
}

/// Alias for strategy coder results.
pub type Result<T> = std::result::Result<T, StrategyCoderError>;

/// Generates strategy source code from a hypothesis using an LLM.
pub struct StrategyCoder {
    llm: Arc<dyn LlmClient>,
}

impl StrategyCoder {
    /// Create a new strategy coder backed by the given LLM client.
    pub fn new(llm: Arc<dyn LlmClient>) -> Self { Self { llm } }

    /// Generate strategy code based on a hypothesis and additional context.
    ///
    /// The generated code must conform to the rara-strategies v2 format:
    /// - `pub fn meta() -> StrategyMeta`
    /// - `pub fn on_candles(candles: &[Candle]) -> StrategyOutput`
    pub async fn generate_code(&self, hypothesis: &Hypothesis, context: &str) -> Result<String> {
        let prompt = format!(
            r#"Generate a Rust trading strategy module that implements the rara-strategies v2 API.

## Required interface

The module must define exactly two public functions:

```rust
pub fn meta() -> StrategyMeta {{
    StrategyMeta {{
        name: "<strategy-name>".into(),
        version: 1,
        api_version: API_VERSION,
        description: "<brief description>".into(),
    }}
}}

pub fn on_candles(candles: &[Candle]) -> StrategyOutput {{
    // Analyze candles and return a score + factor values
    StrategyOutput {{
        score: 0.0,   // -1.0 (bearish) to +1.0 (bullish)
        factors: BTreeMap::new(),  // named factor values
    }}
}}
```

## Available imports

```rust
use std::collections::BTreeMap;
use factor_lib::{{Factor, sma::SmaFactor, ema::EmaFactor, rsi::RsiFactor, ...}};
use strategy_api::{{API_VERSION, Candle, StrategyMeta, StrategyOutput}};
```

## Available factors from `factor_lib`

- `SmaFactor::new(period)` — Simple Moving Average
- `EmaFactor::new(period)` — Exponential Moving Average
- `RsiFactor::new(period)` — Relative Strength Index
- `MomentumFactor::new(period)` — Price momentum
- `VolatilityFactor::new(period)` — Volatility (std dev)
- `VolumeFactor::new(period)` — Volume moving average
- `MeanReversionFactor::new(period)` — Mean reversion z-score

All factors implement `fn last(&self, candles: &[Candle]) -> f64` and
`fn compute(&self, candles: &[Candle]) -> Vec<f64>`.

## Hypothesis

{hypothesis_text}

Reason: {hypothesis_reason}

## Context

{context}

## Rules

- Return ONLY the Rust code (no markdown fences, no explanation)
- The score must be in [-1.0, 1.0]
- Handle insufficient data gracefully (return score 0.0 with empty factors)
- Include all necessary `use` statements at the top
- Do NOT define main() or lib-level attributes
"#,
            hypothesis_text = hypothesis.text,
            hypothesis_reason = hypothesis.reason,
            context = context,
        );

        self.llm.complete(&prompt).await.context(LlmSnafu)
    }

    /// Ask the LLM to fix compilation errors in previously generated code.
    pub async fn fix_errors(
        &self,
        code: &str,
        errors: &[String],
        hypothesis: &Hypothesis,
    ) -> Result<String> {
        let prompt = format!(
            r"Fix the following Rust strategy code that failed to compile.

## Required interface (rara-strategies v2)

The code must define:
- `pub fn meta() -> StrategyMeta`
- `pub fn on_candles(candles: &[Candle]) -> StrategyOutput`

Use `strategy_api::` (not `rara_strategy_api::`).
Return `StrategyOutput {{ score, factors }}` (not `Signal`).

## Hypothesis

{hypothesis_text}

## Current code

```rust
{code}
```

## Compilation errors

{errors}

Return ONLY the corrected Rust code (no markdown fences, no explanation).
",
            hypothesis_text = hypothesis.text,
            errors = errors.join("\n"),
        );

        self.llm.complete(&prompt).await.context(LlmSnafu)
    }
}

#[cfg(test)]
mod tests {
    use rara_agent::{
        backend::{CliBackend, OutputFormat, PromptMode},
        executor::CliExecutor,
    };

    use super::*;

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
    async fn generate_code_returns_llm_response() {
        let executor = echo_executor("fn strategy() { buy() }");
        let coder = StrategyCoder::new(Arc::new(executor));

        let h = Hypothesis::builder()
            .text("momentum works")
            .reason("historical evidence")
            .build();

        let code = coder.generate_code(&h, "BTC").await.unwrap();
        assert_eq!(code, "fn strategy() { buy() }");
    }
}
