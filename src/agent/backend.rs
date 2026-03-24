//! CLI backend definitions for different AI tools.
//!
//! Provides factory methods for configuring various agent CLIs
//! (Claude, Kiro, Gemini, Codex, etc.) with the correct flags and
//! prompt-passing conventions. Ported from ralph-orchestrator.

use std::io::Write;

use snafu::Snafu;
use tempfile::NamedTempFile;

use super::config::AgentConfig;

/// Module-level result type.
pub type Result<T> = std::result::Result<T, BackendError>;

/// Prompts longer than this are written to a temp file and the agent is
/// asked to read from that file instead. This avoids OS `ARG_MAX` limits.
const LARGE_PROMPT_THRESHOLD: usize = 7000;

/// Output format supported by a CLI backend.
///
/// This allows adapters to declare whether they emit structured JSON
/// for real-time streaming or plain text output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OutputFormat {
    /// Plain text output (default for most adapters).
    #[default]
    Text,
    /// Newline-delimited JSON stream (Claude with `--output-format stream-json`).
    StreamJson,
    /// Newline-delimited JSON stream (Pi with `--mode json`).
    PiStreamJson,
    /// Agent Client Protocol over stdio (Kiro v2).
    Acp,
}

/// Errors that can occur when constructing a backend.
#[derive(Debug, Snafu)]
#[snafu(visibility(pub))]
pub enum BackendError {
    /// Custom backend requires a command to be specified.
    #[snafu(display("custom backend requires a command to be specified"))]
    CustomBackendRequiresCommand,

    /// Unknown backend name.
    #[snafu(display("unknown backend: {name}"))]
    UnknownBackend {
        /// The unrecognized backend name.
        name: String,
    },
}

/// How to pass prompts to the CLI tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptMode {
    /// Pass prompt as a command-line argument.
    Arg,
    /// Write prompt to stdin.
    Stdin,
}

/// Prepared command ready for execution.
///
/// Returned by [`CliBackend::build_command`]. The `_temp_file` handle must
/// be kept alive for the duration of command execution — dropping it deletes
/// the underlying file.
pub struct CommandSpec {
    /// The command binary to execute.
    pub command: String,
    /// Fully resolved argument list (includes prompt).
    pub args: Vec<String>,
    /// If set, this string should be written to the child's stdin.
    pub stdin_input: Option<String>,
    /// Temp file handle — kept alive so the agent can read from it.
    _temp_file: Option<NamedTempFile>,
}

/// A CLI backend configuration for executing prompts.
///
/// Factory methods construct backends with the correct flags for each
/// supported agent CLI. All prompts are passed to CLI processes directly
/// via [`std::process::Command`] (no shell involved), so there is no
/// shell injection risk. However, callers should treat this as a local
/// execution boundary — only pass prompts from trusted, local sources.
#[derive(Debug, Clone)]
pub struct CliBackend {
    /// The command to execute.
    pub command: String,
    /// Additional arguments before the prompt.
    pub args: Vec<String>,
    /// How to pass the prompt.
    pub prompt_mode: PromptMode,
    /// Argument flag for prompt (if `prompt_mode` is `Arg`).
    pub prompt_flag: Option<String>,
    /// Output format emitted by this backend.
    pub output_format: OutputFormat,
    /// Environment variables to set when spawning the process.
    pub env_vars: Vec<(String, String)>,
}

impl CliBackend {
    /// Creates a backend from an [`AgentConfig`].
    ///
    /// Delegates to [`Self::from_name`] for named backends and applies
    /// config overrides (extra args, command path).
    ///
    /// # Errors
    /// Returns [`BackendError`] if the backend is "custom" but no command is
    /// specified, or if the backend name is unrecognized.
    pub fn from_agent_config(config: &AgentConfig) -> Result<Self> {
        if config.backend == "custom" {
            return Self::custom(config);
        }

        let mut backend = Self::from_name(&config.backend)?;

        // Apply configured extra args for named backends too.
        backend.args.extend(config.args.iter().cloned());
        if backend.command == "codex" {
            Self::reconcile_codex_args(&mut backend.args);
        }

        // Honor command override for named backends (e.g., custom binary path)
        if let Some(ref cmd) = config.command {
            backend.command.clone_from(cmd);
        }

        Ok(backend)
    }

    /// Creates the Claude backend.
    ///
    /// Uses `-p` flag for headless/print mode execution. This runs Claude
    /// in non-interactive mode where it executes the prompt and exits.
    ///
    /// Emits `--output-format stream-json` for NDJSON streaming output.
    /// Note: `--verbose` is required when using `--output-format stream-json` with `-p`.
    pub fn claude() -> Self {
        Self {
            command: "claude".to_string(),
            args: vec![
                "--dangerously-skip-permissions".to_string(),
                "--verbose".to_string(),
                "--output-format".to_string(),
                "stream-json".to_string(),
            ],
            prompt_mode: PromptMode::Arg,
            prompt_flag: Some("-p".to_string()),
            output_format: OutputFormat::StreamJson,
            env_vars: vec![],
        }
    }

    /// Creates the Claude backend for interactive prompt injection.
    ///
    /// Runs Claude without `-p` flag, passing prompt as a positional argument.
    pub fn claude_interactive() -> Self {
        Self {
            command: "claude".to_string(),
            args: vec!["--dangerously-skip-permissions".to_string()],
            prompt_mode: PromptMode::Arg,
            prompt_flag: None,
            output_format: OutputFormat::Text,
            env_vars: vec![],
        }
    }

    /// Creates the Claude interactive backend with Agent Teams support.
    ///
    /// Like `claude_interactive()` but with
    /// `CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS=1` env var.
    pub fn claude_interactive_teams() -> Self {
        Self {
            command: "claude".to_string(),
            args: vec!["--dangerously-skip-permissions".to_string()],
            prompt_mode: PromptMode::Arg,
            prompt_flag: None,
            output_format: OutputFormat::Text,
            env_vars: vec![(
                "CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS".to_string(),
                "1".to_string(),
            )],
        }
    }

    /// Creates the Kiro backend.
    ///
    /// Uses `kiro-cli` in headless mode with all tools trusted.
    pub fn kiro() -> Self {
        Self {
            command: "kiro-cli".to_string(),
            args: vec![
                "chat".to_string(),
                "--no-interactive".to_string(),
                "--trust-all-tools".to_string(),
            ],
            prompt_mode: PromptMode::Arg,
            prompt_flag: None,
            output_format: OutputFormat::Text,
            env_vars: vec![],
        }
    }

    /// Creates the Kiro backend with a specific agent and optional extra args.
    ///
    /// Uses `kiro-cli` with `--agent` flag to select a specific agent.
    pub fn kiro_with_agent(agent: String, extra_args: &[String]) -> Self {
        let mut backend = Self {
            command: "kiro-cli".to_string(),
            args: vec![
                "chat".to_string(),
                "--no-interactive".to_string(),
                "--trust-all-tools".to_string(),
                "--agent".to_string(),
                agent,
            ],
            prompt_mode: PromptMode::Arg,
            prompt_flag: None,
            output_format: OutputFormat::Text,
            env_vars: vec![],
        };
        backend.args.extend(extra_args.iter().cloned());
        backend
    }

    /// Creates the Kiro ACP backend.
    ///
    /// Uses `kiro-cli` with the ACP subcommand for structured JSON-RPC
    /// communication over stdio instead of PTY text scraping.
    pub fn kiro_acp() -> Self {
        Self::kiro_acp_with_options(None, None)
    }

    /// Creates the Kiro ACP backend with an optional agent and/or model.
    pub fn kiro_acp_with_options(agent: Option<&str>, model: Option<&str>) -> Self {
        let mut args = vec!["acp".to_string()];
        if let Some(name) = agent {
            args.push("--agent".to_string());
            args.push(name.to_string());
        }
        if let Some(m) = model {
            args.push("--model".to_string());
            args.push(m.to_string());
        }
        Self {
            command: "kiro-cli".to_string(),
            args,
            prompt_mode: PromptMode::Stdin,
            prompt_flag: None,
            output_format: OutputFormat::Acp,
            env_vars: vec![],
        }
    }

    /// Creates the Gemini backend.
    pub fn gemini() -> Self {
        Self {
            command: "gemini".to_string(),
            args: vec!["--yolo".to_string()],
            prompt_mode: PromptMode::Arg,
            prompt_flag: Some("-p".to_string()),
            output_format: OutputFormat::Text,
            env_vars: vec![],
        }
    }

    /// Gemini in interactive mode with initial prompt (uses `-i`, not `-p`).
    ///
    /// **Critical quirk**: Gemini requires `-i` flag for interactive+prompt mode.
    /// Using `-p` would make it run headless and exit after one response.
    pub fn gemini_interactive() -> Self {
        Self {
            command: "gemini".to_string(),
            args: vec!["--yolo".to_string()],
            prompt_mode: PromptMode::Arg,
            prompt_flag: Some("-i".to_string()),
            output_format: OutputFormat::Text,
            env_vars: vec![],
        }
    }

    /// Creates the Codex backend.
    pub fn codex() -> Self {
        Self {
            command: "codex".to_string(),
            args: vec!["exec".to_string(), "--yolo".to_string()],
            prompt_mode: PromptMode::Arg,
            prompt_flag: None,
            output_format: OutputFormat::Text,
            env_vars: vec![],
        }
    }

    /// Codex in interactive TUI mode (no `exec` subcommand).
    ///
    /// Unlike headless `codex()`, this runs without `exec` and `--full-auto`
    /// flags, allowing interactive TUI mode.
    pub fn codex_interactive() -> Self {
        Self {
            command: "codex".to_string(),
            args: vec![],
            prompt_mode: PromptMode::Arg,
            prompt_flag: None,
            output_format: OutputFormat::Text,
            env_vars: vec![],
        }
    }

    /// Creates the Amp backend.
    pub fn amp() -> Self {
        Self {
            command: "amp".to_string(),
            args: vec!["--dangerously-allow-all".to_string()],
            prompt_mode: PromptMode::Arg,
            prompt_flag: Some("-x".to_string()),
            output_format: OutputFormat::Text,
            env_vars: vec![],
        }
    }

    /// Amp in interactive mode (removes `--dangerously-allow-all`).
    ///
    /// Unlike headless `amp()`, this runs without the auto-approve flag,
    /// requiring user confirmation for tool usage.
    pub fn amp_interactive() -> Self {
        Self {
            command: "amp".to_string(),
            args: vec![],
            prompt_mode: PromptMode::Arg,
            prompt_flag: Some("-x".to_string()),
            output_format: OutputFormat::Text,
            env_vars: vec![],
        }
    }

    /// Creates the Copilot backend for autonomous mode.
    ///
    /// Uses GitHub Copilot CLI with `--allow-all-tools` for automated tool approval.
    pub fn copilot() -> Self {
        Self {
            command: "copilot".to_string(),
            args: vec!["--allow-all-tools".to_string()],
            prompt_mode: PromptMode::Arg,
            prompt_flag: Some("-p".to_string()),
            output_format: OutputFormat::Text,
            env_vars: vec![],
        }
    }

    /// Copilot in interactive mode (removes `--allow-all-tools`).
    ///
    /// Unlike headless `copilot()`, this runs without the auto-approve flag,
    /// requiring user confirmation for tool usage.
    pub fn copilot_interactive() -> Self {
        Self {
            command: "copilot".to_string(),
            args: vec![],
            prompt_mode: PromptMode::Arg,
            prompt_flag: Some("-p".to_string()),
            output_format: OutputFormat::Text,
            env_vars: vec![],
        }
    }

    /// Creates the Copilot TUI backend for interactive mode.
    ///
    /// Runs Copilot in full interactive mode (no `-p` flag), allowing
    /// Copilot's native TUI to render.
    pub fn copilot_tui() -> Self {
        Self {
            command: "copilot".to_string(),
            args: vec![],
            prompt_mode: PromptMode::Arg,
            prompt_flag: None,
            output_format: OutputFormat::Text,
            env_vars: vec![],
        }
    }

    /// Creates the `OpenCode` backend for autonomous/headless mode.
    ///
    /// Uses `OpenCode` CLI with `run` subcommand. The prompt is passed as a
    /// positional argument after the subcommand.
    pub fn opencode() -> Self {
        Self {
            command: "opencode".to_string(),
            args: vec!["run".to_string()],
            prompt_mode: PromptMode::Arg,
            prompt_flag: None,
            output_format: OutputFormat::Text,
            env_vars: vec![],
        }
    }

    /// `OpenCode` in interactive TUI mode.
    ///
    /// Runs `OpenCode` TUI with an initial prompt via `--prompt` flag.
    /// Unlike `opencode()` which uses `opencode run` (headless mode),
    /// this launches the interactive TUI and injects the prompt.
    pub fn opencode_interactive() -> Self {
        Self {
            command: "opencode".to_string(),
            args: vec![],
            prompt_mode: PromptMode::Arg,
            prompt_flag: Some("--prompt".to_string()),
            output_format: OutputFormat::Text,
            env_vars: vec![],
        }
    }

    /// Creates the Pi backend for headless execution.
    ///
    /// Uses `-p` for print mode with `--mode json` for NDJSON streaming output.
    /// Emits `PiStreamJson` output format for structured event parsing.
    pub fn pi() -> Self {
        Self {
            command: "pi".to_string(),
            args: vec![
                "-p".to_string(),
                "--mode".to_string(),
                "json".to_string(),
                "--no-session".to_string(),
            ],
            prompt_mode: PromptMode::Arg,
            prompt_flag: None,
            output_format: OutputFormat::PiStreamJson,
            env_vars: vec![],
        }
    }

    /// Creates the Pi backend for interactive mode with initial prompt.
    ///
    /// Runs pi TUI without `-p` or `--mode json`, passing the prompt as a
    /// positional argument.
    pub fn pi_interactive() -> Self {
        Self {
            command: "pi".to_string(),
            args: vec!["--no-session".to_string()],
            prompt_mode: PromptMode::Arg,
            prompt_flag: None,
            output_format: OutputFormat::Text,
            env_vars: vec![],
        }
    }

    /// Creates the Roo backend for headless execution.
    ///
    /// Uses `--print` for non-interactive output and `--ephemeral` for clean
    /// disk state. Prompts are always passed via `--prompt-file` (handled in
    /// `build_command()`).
    pub fn roo() -> Self {
        Self {
            command: "roo".to_string(),
            args: vec!["--print".to_string(), "--ephemeral".to_string()],
            prompt_mode: PromptMode::Arg,
            prompt_flag: None,
            output_format: OutputFormat::Text,
            env_vars: vec![],
        }
    }

    /// Creates the Roo backend for interactive mode with initial prompt.
    ///
    /// Runs roo TUI without `--print` or `--ephemeral`, passing the prompt
    /// as a positional argument.
    pub fn roo_interactive() -> Self {
        Self {
            command: "roo".to_string(),
            args: vec![],
            prompt_mode: PromptMode::Arg,
            prompt_flag: None,
            output_format: OutputFormat::Text,
            env_vars: vec![],
        }
    }

    /// Creates a custom backend from configuration.
    ///
    /// # Errors
    /// Returns [`BackendError::CustomBackendRequiresCommand`] if no command is specified.
    pub fn custom(config: &AgentConfig) -> Result<Self> {
        let command = config
            .command
            .clone()
            .ok_or(BackendError::CustomBackendRequiresCommand)?;
        let prompt_mode = match config.prompt_mode {
            super::config::ConfigPromptMode::Stdin => PromptMode::Stdin,
            super::config::ConfigPromptMode::Arg => PromptMode::Arg,
        };

        Ok(Self {
            command,
            args: config.args.clone(),
            prompt_mode,
            prompt_flag: config.prompt_flag.clone(),
            output_format: OutputFormat::Text,
            env_vars: vec![],
        })
    }

    /// Creates a backend from a named backend string.
    ///
    /// # Errors
    /// Returns [`BackendError::UnknownBackend`] if the name is not recognized.
    pub fn from_name(name: &str) -> Result<Self> {
        match name {
            "claude" => Ok(Self::claude()),
            "kiro" => Ok(Self::kiro()),
            "kiro-acp" => Ok(Self::kiro_acp()),
            "gemini" => Ok(Self::gemini()),
            "codex" => Ok(Self::codex()),
            "amp" => Ok(Self::amp()),
            "copilot" => Ok(Self::copilot()),
            "opencode" => Ok(Self::opencode()),
            "pi" => Ok(Self::pi()),
            "roo" => Ok(Self::roo()),
            _ => UnknownBackendSnafu {
                name: name.to_string(),
            }
            .fail(),
        }
    }

    /// Creates a backend from a named backend with additional args.
    ///
    /// # Errors
    /// Returns error if the backend name is invalid.
    pub fn from_name_with_args(
        name: &str,
        extra_args: &[String],
    ) -> Result<Self> {
        let mut backend = Self::from_name(name)?;
        backend.args.extend(extra_args.iter().cloned());
        if backend.command == "codex" {
            Self::reconcile_codex_args(&mut backend.args);
        }
        Ok(backend)
    }

    /// Creates a backend configured for interactive mode with initial prompt.
    ///
    /// Returns the correct backend configuration for running an interactive
    /// session with an initial prompt.
    ///
    /// # Errors
    /// Returns [`BackendError::UnknownBackend`] if the backend name is not recognized.
    pub fn for_interactive_prompt(backend_name: &str) -> Result<Self> {
        match backend_name {
            "claude" => Ok(Self::claude_interactive()),
            "kiro" => Ok(Self::kiro_interactive()),
            "gemini" => Ok(Self::gemini_interactive()),
            "codex" => Ok(Self::codex_interactive()),
            "amp" => Ok(Self::amp_interactive()),
            "copilot" => Ok(Self::copilot_interactive()),
            "opencode" => Ok(Self::opencode_interactive()),
            "pi" => Ok(Self::pi_interactive()),
            "roo" => Ok(Self::roo_interactive()),
            _ => UnknownBackendSnafu {
                name: backend_name.to_string(),
            }
            .fail(),
        }
    }

    /// Kiro in interactive mode (removes `--no-interactive`).
    ///
    /// Unlike headless `kiro()`, this allows the user to interact with
    /// Kiro's TUI while still passing an initial prompt.
    pub fn kiro_interactive() -> Self {
        Self {
            command: "kiro-cli".to_string(),
            args: vec!["chat".to_string(), "--trust-all-tools".to_string()],
            prompt_mode: PromptMode::Arg,
            prompt_flag: None,
            output_format: OutputFormat::Text,
            env_vars: vec![],
        }
    }

    /// Builds roo prompt-file args: writes prompt to a temp file and
    /// appends `--prompt-file <path>` to args. Falls back to positional
    /// arg if temp file creation fails.
    fn build_roo_prompt_file(
        args: &mut Vec<String>,
        prompt: &str,
    ) -> (Option<String>, Option<NamedTempFile>) {
        match NamedTempFile::new() {
            Ok(mut file) => {
                if let Err(e) = file.write_all(prompt.as_bytes()) {
                    tracing::warn!("Failed to write roo prompt to temp file: {e}");
                    args.push(prompt.to_string());
                    (None, None)
                } else {
                    args.push("--prompt-file".to_string());
                    args.push(file.path().display().to_string());
                    (None, Some(file))
                }
            }
            Err(e) => {
                tracing::warn!("Failed to create temp file for roo: {e}");
                args.push(prompt.to_string());
                (None, None)
            }
        }
    }

    /// Builds the full command with arguments for execution.
    ///
    /// # Safety assumptions
    ///
    /// The prompt is passed directly to the child process via
    /// [`std::process::Command`] — no shell is involved, so there is no
    /// shell-injection risk. This function is intended for local, trusted
    /// prompts only. Do not pass untrusted external input without
    /// validation.
    pub fn build_command(&self, prompt: &str, interactive: bool) -> CommandSpec {
        let mut args = self.args.clone();

        // Filter args based on execution mode
        if interactive {
            args = self.filter_args_for_interactive(args);
        }

        // Handle prompt passing: Roo uses --prompt-file, all others use temp file for large prompts
        let (stdin_input, temp_file) = match self.prompt_mode {
            PromptMode::Arg => {
                // Roo headless: always use --prompt-file for all prompts
                if self.command == "roo" && args.contains(&"--print".to_string()) {
                    Self::build_roo_prompt_file(&mut args, prompt)
                } else {
                    let (prompt_text, temp_file) = if prompt.len() > LARGE_PROMPT_THRESHOLD {
                        match NamedTempFile::new() {
                            Ok(mut file) => {
                                if let Err(e) = file.write_all(prompt.as_bytes()) {
                                    tracing::warn!(
                                        "Failed to write prompt to temp file: {e}"
                                    );
                                    (prompt.to_string(), None)
                                } else {
                                    let path = file.path().display().to_string();
                                    (
                                        format!(
                                            "Please read and execute the task in {path}"
                                        ),
                                        Some(file),
                                    )
                                }
                            }
                            Err(e) => {
                                tracing::warn!("Failed to create temp file: {e}");
                                (prompt.to_string(), None)
                            }
                        }
                    } else {
                        (prompt.to_string(), None)
                    };

                    if let Some(ref flag) = self.prompt_flag {
                        args.push(flag.clone());
                    }
                    args.push(prompt_text);
                    (None, temp_file)
                }
            }
            PromptMode::Stdin => (Some(prompt.to_string()), None),
        };

        tracing::debug!(
            command = %self.command,
            args_count = args.len(),
            prompt_len = prompt.len(),
            interactive = interactive,
            uses_stdin = stdin_input.is_some(),
            uses_temp_file = temp_file.is_some(),
            "Built CLI command"
        );
        tracing::trace!(prompt = %prompt, "Full prompt content");

        CommandSpec {
            command: self.command.clone(),
            args,
            stdin_input,
            _temp_file: temp_file,
        }
    }

    /// Filters args for interactive mode per spec table.
    fn filter_args_for_interactive(&self, args: Vec<String>) -> Vec<String> {
        match self.command.as_str() {
            "kiro-cli" => args
                .into_iter()
                .filter(|a| a != "--no-interactive")
                .collect(),
            "codex" => args.into_iter().filter(|a| a != "--full-auto").collect(),
            "amp" => args
                .into_iter()
                .filter(|a| a != "--dangerously-allow-all")
                .collect(),
            "copilot" => args
                .into_iter()
                .filter(|a| a != "--allow-all-tools")
                .collect(),
            "roo" => args
                .into_iter()
                .filter(|a| a != "--print" && a != "--ephemeral")
                .collect(),
            _ => args, // claude, gemini, opencode unchanged
        }
    }

    /// Reconciles codex args to resolve conflicting flags.
    ///
    /// Replaces deprecated `--dangerously-bypass-approvals-and-sandbox` with
    /// `--yolo`, removes `--full-auto` when `--yolo` is present, and
    /// deduplicates `--yolo` entries.
    fn reconcile_codex_args(args: &mut Vec<String>) {
        let had_dangerous_bypass = args
            .iter()
            .any(|arg| arg == "--dangerously-bypass-approvals-and-sandbox");
        if had_dangerous_bypass {
            args.retain(|arg| arg != "--dangerously-bypass-approvals-and-sandbox");
            if !args.iter().any(|arg| arg == "--yolo") {
                if let Some(pos) = args.iter().position(|arg| arg == "exec") {
                    args.insert(pos + 1, "--yolo".to_string());
                } else {
                    args.push("--yolo".to_string());
                }
            }
        }

        if args.iter().any(|arg| arg == "--yolo") {
            args.retain(|arg| arg != "--full-auto");
            // Collapse duplicate --yolo entries to a single flag.
            let mut seen_yolo = false;
            args.retain(|arg| {
                if arg == "--yolo" {
                    if seen_yolo {
                        return false;
                    }
                    seen_yolo = true;
                }
                true
            });
        }
    }
}
