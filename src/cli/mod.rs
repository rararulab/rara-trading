//! CLI command definitions and subcommand modules.

use clap::{Parser, Subcommand};

/// Your CLI application — update this doc comment.
#[derive(Parser)]
#[command(name = "rara-trading", version)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

/// Available subcommands.
#[derive(Subcommand)]
pub enum Command {
    /// Say hello (example command — replace with your own)
    Hello {
        /// Name to greet
        #[arg(default_value = "world")]
        name: String,
    },

    /// Manage config values
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },

    /// Run a prompt through the configured agent backend
    Agent {
        /// The prompt to send to the agent
        prompt: String,
        /// Override the backend (e.g., "claude", "codex")
        #[arg(long)]
        backend: Option<String>,
    },
}

/// Config management subcommands.
#[derive(Subcommand)]
pub enum ConfigAction {
    /// Set a config value
    Set {
        /// Config key (e.g. example.setting)
        key:   String,
        /// Config value
        value: String,
    },
    /// Get a config value
    Get {
        /// Config key to look up
        key: String,
    },
    /// List all config values
    List,
}
