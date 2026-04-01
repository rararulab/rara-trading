//! Compiles LLM-generated Rust strategy code into WASM modules via the
//! `rara-strategy` CLI.
//!
//! Workflow: write the generated code to `{strategies_dir}/strategies/{name}/src/logic.rs`,
//! invoke `rara-strategy build <name>`, and read the resulting WASM artifact.

use std::path::PathBuf;

use bon::Builder;
use serde::Deserialize;
use snafu::{ResultExt, Snafu};

/// Errors from strategy compilation.
#[derive(Debug, Snafu)]
#[snafu(visibility(pub))]
pub enum CompilerError {
    /// rara-strategy CLI binary not found.
    #[snafu(display("rara-strategy CLI not found (tried {path} and PATH)"))]
    CliNotFound {
        /// The local path that was checked first.
        path: String,
    },

    /// Failed to execute the CLI process.
    #[snafu(display("CLI execution failed: {source}"))]
    CliExecution { source: std::io::Error },

    /// Failed to parse JSON output from the CLI.
    #[snafu(display("failed to parse CLI output: {source}"))]
    ParseOutput { source: serde_json::Error },

    /// Failed to write strategy code to the target file.
    #[snafu(display("failed to write strategy code: {source}"))]
    WriteCode { source: std::io::Error },

    /// Failed to read compiled WASM binary.
    #[snafu(display("failed to read WASM output: {source}"))]
    ReadWasm { source: std::io::Error },

    /// The CLI create command failed.
    #[snafu(display("strategy creation failed: {message}"))]
    CreateFailed {
        /// Error details from the CLI.
        message: String,
    },
}

/// Module-level result alias.
pub type Result<T> = std::result::Result<T, CompilerError>;

/// Result of a strategy compilation attempt.
#[derive(Debug)]
pub struct CompileResult {
    /// Whether compilation succeeded.
    pub success:         bool,
    /// Compiled WASM bytes (if successful).
    pub wasm_bytes:      Option<Vec<u8>>,
    /// Compilation errors from the CLI.
    pub errors:          Vec<String>,
    /// Warnings (currently unused by CLI, reserved for future use).
    pub warnings:        Vec<String>,
    /// Time taken to compile in milliseconds.
    pub compile_time_ms: u64,
}

/// JSON output from `rara-strategy build`.
#[derive(Deserialize)]
struct BuildOutput {
    results:   Vec<BuildResult>,
    #[allow(dead_code)]
    succeeded: usize,
    #[allow(dead_code)]
    failed:    usize,
}

/// A single strategy build result from CLI JSON.
#[derive(Deserialize)]
struct BuildResult {
    #[allow(dead_code)]
    strategy:  String,
    success:   bool,
    wasm_path: Option<String>,
    error:     Option<String>,
}

/// JSON output from `rara-strategy create`.
#[derive(Deserialize)]
struct CreateOutput {
    created: bool,
    path:    String,
    reason:  Option<String>,
}

/// Compiles LLM-generated Rust strategy code to WASM via the rara-strategy CLI.
///
/// Uses a scratch strategy directory (`_scratch` by default) inside the
/// rara-strategies workspace. The LLM-generated code is written to
/// `strategies/{scratch_name}/src/logic.rs`, then `rara-strategy build` is
/// invoked to produce the WASM artifact.
#[derive(Debug, Builder)]
pub struct StrategyCompiler {
    /// Path to the rara-strategies workspace root.
    pub strategies_dir: PathBuf,
    /// Name of the scratch strategy used for compilation.
    /// Defaults to `"_scratch"`.
    #[builder(default = "_scratch".into())]
    pub scratch_name:   String,
}

impl StrategyCompiler {
    /// Compile strategy code to WASM.
    ///
    /// Writes the code to the scratch strategy's `src/logic.rs`, invokes
    /// `rara-strategy build`, and reads the resulting WASM binary.
    pub async fn compile(&self, strategy_code: &str) -> Result<CompileResult> {
        let start = std::time::Instant::now();

        // Ensure scratch strategy exists
        self.ensure_scratch_strategy().await?;

        // Write generated code to logic.rs
        let logic_path = self
            .strategies_dir
            .join("strategies")
            .join(&self.scratch_name)
            .join("src/logic.rs");
        std::fs::write(&logic_path, strategy_code).context(WriteCodeSnafu)?;

        // Run rara-strategy build <scratch_name>
        let cli = self.resolve_cli()?;
        let output = tokio::process::Command::new(&cli)
            .args(["build", &self.scratch_name])
            .current_dir(&self.strategies_dir)
            .output()
            .await
            .context(CliExecutionSnafu)?;

        let compile_time_ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);

        // Parse JSON from stdout
        let stdout = String::from_utf8_lossy(&output.stdout);
        let build_output: BuildOutput =
            serde_json::from_str(&stdout).context(ParseOutputSnafu)?;

        // Extract result for our scratch strategy
        let result = build_output
            .results
            .into_iter()
            .find(|r| r.strategy == self.scratch_name)
            .unwrap_or_else(|| BuildResult {
                strategy:  self.scratch_name.clone(),
                success:   false,
                wasm_path: None,
                error:     Some("strategy not found in build output".into()),
            });

        if !result.success {
            let errors = result
                .error
                .map(|e| vec![e])
                .unwrap_or_default();
            return Ok(CompileResult {
                success: false,
                wasm_bytes: None,
                errors,
                warnings: vec![],
                compile_time_ms,
            });
        }

        // Read WASM bytes from the path reported by the CLI
        let wasm_path = result
            .wasm_path
            .expect("success implies wasm_path is present");
        let wasm_bytes = std::fs::read(&wasm_path).context(ReadWasmSnafu)?;

        Ok(CompileResult {
            success: true,
            wasm_bytes: Some(wasm_bytes),
            errors: vec![],
            warnings: vec![],
            compile_time_ms,
        })
    }

    /// Create a new named strategy in the rara-strategies workspace.
    ///
    /// Calls `rara-strategy create <name> --description "..."`. Idempotent:
    /// returns `Ok` if the strategy already exists.
    pub async fn create_strategy(&self, name: &str, description: &str) -> Result<PathBuf> {
        let cli = self.resolve_cli()?;
        let output = tokio::process::Command::new(&cli)
            .args(["create", name, "--description", description])
            .current_dir(&self.strategies_dir)
            .output()
            .await
            .context(CliExecutionSnafu)?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let create_output: CreateOutput =
            serde_json::from_str(&stdout).context(ParseOutputSnafu)?;

        if !create_output.created && create_output.reason.as_deref() != Some("already exists") {
            return Err(CompilerError::CreateFailed {
                message: create_output
                    .reason
                    .unwrap_or_else(|| "unknown error".into()),
            });
        }

        Ok(PathBuf::from(create_output.path))
    }

    /// Ensure the scratch strategy directory exists, creating it if needed.
    async fn ensure_scratch_strategy(&self) -> Result<()> {
        let scratch_dir = self
            .strategies_dir
            .join("strategies")
            .join(&self.scratch_name);
        if !scratch_dir.exists() {
            self.create_strategy(&self.scratch_name, "Scratch strategy for LLM compilation")
                .await?;
        }
        Ok(())
    }

    /// Resolve the rara-strategy CLI binary path.
    ///
    /// Checks `{strategies_dir}/target/release/rara-strategy-cli` first,
    /// then falls back to `rara-strategy` on PATH.
    fn resolve_cli(&self) -> Result<PathBuf> {
        let local = self
            .strategies_dir
            .join("target/release/rara-strategy-cli");
        if local.exists() {
            return Ok(local);
        }

        // Fall back to PATH lookup
        which_in_path("rara-strategy").ok_or_else(|| CompilerError::CliNotFound {
            path: local.display().to_string(),
        })
    }
}

/// Look up a binary on PATH, returning its absolute path if found.
fn which_in_path(name: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths).find_map(|dir| {
            let candidate = dir.join(name);
            candidate.is_file().then_some(candidate)
        })
    })
}
