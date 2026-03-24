//! Agent backend for invoking local AI agent CLIs.
//!
//! This module provides a CLI-agnostic execution layer that can invoke
//! various agent CLIs (Claude, Kiro, Gemini, Codex, etc.) with prompts
//! and collect their output. Ported from ralph-orchestrator.

pub mod backend;
pub mod config;
pub mod executor;

pub use backend::{CliBackend, CommandSpec, OutputFormat, PromptMode};
pub use config::{AgentConfig, ConfigPromptMode};
pub use executor::{CliExecutor, ExecutionResult};
