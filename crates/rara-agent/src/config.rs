//! Agent configuration for the TOML `[agent]` section.

use serde::{Deserialize, Serialize};

/// How to pass prompts to the CLI tool (config-level enum).
///
/// Serializes as lowercase strings ("arg" / "stdin") for TOML compatibility.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConfigPromptMode {
    /// Pass prompt as a command-line argument.
    #[default]
    Arg,
    /// Write prompt to stdin.
    Stdin,
}

/// Agent backend configuration.
///
/// Controls which agent CLI to use and how to invoke it.
/// Stored in the `[agent]` section of the config file.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
#[derive(bon::Builder)]
pub struct AgentConfig {
    /// Backend to use: "claude", "kiro", "gemini", "codex", "amp",
    /// "copilot", "opencode", "pi", "roo", or "custom".
    #[builder(default = "claude".to_string())]
    pub backend: String,

    /// Command override. Required for "custom" backend.
    /// For named backends, overrides the default binary path.
    #[builder(into)]
    pub command: Option<String>,

    /// Additional arguments to pass to the CLI command.
    #[builder(default)]
    pub args: Vec<String>,

    /// How to pass prompts: "arg" or "stdin".
    #[builder(default)]
    pub prompt_mode: ConfigPromptMode,

    /// Custom prompt flag for arg mode (e.g., "-p").
    /// If None, uses the backend's default.
    #[builder(into)]
    pub prompt_flag: Option<String>,

    /// Idle timeout in seconds. Process is terminated after this many
    /// seconds of inactivity (no output). Set to 0 to disable.
    #[builder(default = 30)]
    pub idle_timeout_secs: u32,
}

impl Default for AgentConfig {
    fn default() -> Self { Self::builder().build() }
}
