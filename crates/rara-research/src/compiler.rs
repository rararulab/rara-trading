//! Compiles LLM-generated Rust strategy code into WASM modules.
//!
//! Workflow: copy the `strategies/template/` scaffold to a temp directory,
//! inject the generated code between the `STRATEGY_IMPL` markers, then
//! invoke `cargo build --release --target wasm32-wasip1`.

use std::path::{Path, PathBuf};

use bon::Builder;
use snafu::{ResultExt, Snafu};

/// Errors from strategy compilation.
#[derive(Debug, Snafu)]
#[snafu(visibility(pub))]
pub enum CompilerError {
    /// Failed to set up compilation workspace.
    #[snafu(display("workspace setup failed: {source}"))]
    WorkspaceSetup { source: std::io::Error },

    /// Failed to run cargo command.
    #[snafu(display("cargo command failed: {source}"))]
    CargoCommand { source: std::io::Error },

    /// Failed to read compiled WASM binary.
    #[snafu(display("failed to read WASM output: {source}"))]
    ReadWasm { source: std::io::Error },
}

/// Module-level result alias.
pub type Result<T> = std::result::Result<T, CompilerError>;

/// Result of a strategy compilation attempt.
#[derive(Debug)]
pub struct CompileResult {
    /// Whether compilation succeeded.
    pub success: bool,
    /// Compiled WASM bytes (if successful).
    pub wasm_bytes: Option<Vec<u8>>,
    /// Compilation errors from stderr.
    pub errors: Vec<String>,
    /// Clippy warnings from stderr.
    pub warnings: Vec<String>,
    /// Time taken to compile in milliseconds.
    pub compile_time_ms: u64,
}

/// Compiles LLM-generated Rust strategy code to WASM.
#[derive(Debug, Builder)]
pub struct StrategyCompiler {
    /// Path to `strategies/template/` directory.
    template_dir: PathBuf,
    /// WASM target triple.
    #[builder(default = "wasm32-wasip1".into())]
    wasm_target: String,
}

const IMPL_START_MARKER: &str = "// ===== STRATEGY_IMPL START =====";
const IMPL_END_MARKER: &str = "// ===== STRATEGY_IMPL END =====";

impl StrategyCompiler {
    /// Compile strategy code to WASM.
    ///
    /// Copies the template to a temp directory, injects `strategy_code`
    /// between the `STRATEGY_IMPL` markers, patches dependency paths to
    /// absolute form, and runs `cargo build --release`.
    pub async fn compile(&self, strategy_code: &str) -> Result<CompileResult> {
        let start = std::time::Instant::now();

        // 1. Copy template to temp dir
        let tmp = tempfile::tempdir().context(WorkspaceSetupSnafu)?;
        copy_dir_recursive(&self.template_dir, tmp.path()).context(WorkspaceSetupSnafu)?;

        // 2. Patch Cargo.toml to use absolute path for rara-strategy-api
        //    (the relative path breaks once we copy to a temp location)
        let cargo_toml_path = tmp.path().join("Cargo.toml");
        let cargo_toml =
            std::fs::read_to_string(&cargo_toml_path).context(WorkspaceSetupSnafu)?;
        let abs_api_path = std::fs::canonicalize(
            self.template_dir.join("../../crates/rara-strategy-api"),
        )
        .context(WorkspaceSetupSnafu)?;
        let patched = cargo_toml.replace(
            "../../crates/rara-strategy-api",
            &abs_api_path.to_string_lossy(),
        );
        std::fs::write(&cargo_toml_path, &patched).context(WorkspaceSetupSnafu)?;

        // 3. Inject strategy code into template
        let lib_path = tmp.path().join("src/lib.rs");
        let template = std::fs::read_to_string(&lib_path).context(WorkspaceSetupSnafu)?;
        let injected = inject_strategy_code(&template, strategy_code);
        std::fs::write(&lib_path, &injected).context(WorkspaceSetupSnafu)?;

        // 4. Run cargo build --release --target wasm32-wasip1
        let output = tokio::process::Command::new("cargo")
            .args(["build", "--release", "--target", &self.wasm_target])
            .current_dir(tmp.path())
            .output()
            .await
            .context(CargoCommandSnafu)?;

        let stderr = String::from_utf8_lossy(&output.stderr);
        let compile_time_ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);

        if !output.status.success() {
            let errors = parse_cargo_errors(&stderr);
            return Ok(CompileResult {
                success: false,
                wasm_bytes: None,
                errors,
                warnings: vec![],
                compile_time_ms,
            });
        }

        // 5. Read the .wasm file
        let wasm_path = tmp
            .path()
            .join("target")
            .join(&self.wasm_target)
            .join("release")
            .join("generated_strategy.wasm");
        let wasm_bytes = std::fs::read(&wasm_path).context(ReadWasmSnafu)?;

        // 6. Collect warnings
        let warnings = parse_cargo_warnings(&stderr);

        Ok(CompileResult {
            success: true,
            wasm_bytes: Some(wasm_bytes),
            errors: vec![],
            warnings,
            compile_time_ms,
        })
    }
}

/// Replace the `STRATEGY_IMPL` section in the template with generated code.
fn inject_strategy_code(template: &str, strategy_code: &str) -> String {
    let start_idx = template
        .find(IMPL_START_MARKER)
        .expect("template missing STRATEGY_IMPL START marker");
    let end_idx = template
        .find(IMPL_END_MARKER)
        .expect("template missing STRATEGY_IMPL END marker");

    let mut result = String::with_capacity(template.len() + strategy_code.len());
    result.push_str(&template[..start_idx]);
    result.push_str(IMPL_START_MARKER);
    result.push('\n');
    result.push_str(strategy_code);
    result.push('\n');
    result.push_str(&template[end_idx..]);
    result
}

/// Recursively copy a directory, skipping `target/`.
fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if src_path.is_dir() {
            // Skip target directory to avoid copying build artifacts
            if entry.file_name() == "target" {
                continue;
            }
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            std::fs::copy(&src_path, &dst_path)?;
        }
    }
    Ok(())
}

/// Extract error messages from cargo stderr.
fn parse_cargo_errors(stderr: &str) -> Vec<String> {
    stderr
        .lines()
        .filter(|line| line.starts_with("error"))
        .map(String::from)
        .collect()
}

/// Extract warning messages from cargo stderr.
fn parse_cargo_warnings(stderr: &str) -> Vec<String> {
    stderr
        .lines()
        .filter(|line| line.starts_with("warning"))
        .map(String::from)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn template_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../strategies/template")
    }

    #[tokio::test]
    async fn compiles_valid_strategy_to_wasm() {
        let compiler = StrategyCompiler::builder()
            .template_dir(template_dir())
            .build();

        let code = r#"
fn meta() -> StrategyMeta {
    StrategyMeta {
        name: "test-sma".into(),
        version: 1,
        api_version: API_VERSION,
        description: "Simple test strategy".into(),
    }
}

fn on_candles(candles: &[Candle]) -> Signal {
    if candles.len() < 2 {
        return Signal::Hold;
    }
    let last = candles.last().unwrap();
    let prev = &candles[candles.len() - 2];
    if last.close > prev.close {
        Signal::Entry { side: Side::Long, strength: 0.8 }
    } else {
        Signal::Entry { side: Side::Short, strength: 0.8 }
    }
}

fn risk_levels(entry_price: f64, side: Side) -> RiskLevels {
    let offset = entry_price * 0.02;
    match side {
        Side::Long => RiskLevels {
            stop_loss: entry_price - offset,
            take_profit: entry_price + offset,
        },
        Side::Short => RiskLevels {
            stop_loss: entry_price + offset,
            take_profit: entry_price - offset,
        },
    }
}
"#;

        let result = compiler.compile(code).await.expect("compile should not error");
        assert!(result.success, "expected success, got errors: {:?}", result.errors);
        assert!(result.wasm_bytes.is_some(), "expected wasm bytes");
        assert!(
            result.wasm_bytes.as_ref().unwrap().len() > 100,
            "wasm binary too small"
        );
    }

    #[tokio::test]
    async fn returns_errors_for_invalid_code() {
        let compiler = StrategyCompiler::builder()
            .template_dir(template_dir())
            .build();

        let result = compiler
            .compile("fn broken( {")
            .await
            .expect("compile call should not error");
        assert!(!result.success);
        assert!(!result.errors.is_empty(), "expected compilation errors");
    }
}
