//! Agent backend for invoking local AI agent CLIs.
//!
//! Re-exports from the `rara-agent` crate.

pub use rara_agent::backend;
pub use rara_agent::config;
pub use rara_agent::executor;

pub use rara_agent::{
    AgentConfig, CliBackend, CliExecutor, CommandSpec, ConfigPromptMode, ExecutionResult,
    OutputFormat, PromptMode,
};
