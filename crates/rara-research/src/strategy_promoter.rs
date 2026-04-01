//! Strategy promotion — saves accepted WASM strategies for paper trading
//! pickup.
//!
//! When the research loop accepts a candidate strategy, the promoter persists
//! the compiled WASM binary and metadata so downstream systems (paper trading,
//! live trading) can discover and load them.

use std::path::{Path, PathBuf};

use bon::Builder;
use rara_strategy_api::StrategyMeta;
use serde::{Deserialize, Serialize};
use snafu::{ResultExt, Snafu};
use uuid::Uuid;

use crate::{
    compiler::StrategyCompiler, strategy_executor::StrategyExecutor, trace::Trace,
    wasm_executor::WasmExecutor,
};

/// Errors from strategy promotion operations.
#[derive(Debug, Snafu)]
#[snafu(visibility(pub))]
pub enum PromoterError {
    /// Experiment not found in trace.
    #[snafu(display("experiment {id} not found in trace"))]
    ExperimentNotFound {
        /// The missing experiment ID.
        id: Uuid,
    },

    /// Hypothesis not found in trace.
    #[snafu(display("hypothesis {id} not found in trace"))]
    HypothesisNotFound {
        /// The missing hypothesis ID.
        id: Uuid,
    },

    /// Failed to compile strategy code to WASM.
    #[snafu(display("compilation failed: {source}"))]
    Compile {
        /// The underlying compiler error.
        source: crate::compiler::CompilerError,
    },

    /// Compilation produced errors instead of a valid WASM binary.
    #[snafu(display("compilation returned errors: {}", errors.join("; ")))]
    CompileErrors {
        /// Compiler error messages.
        errors: Vec<String>,
    },

    /// WASM runtime validation failed.
    #[snafu(display("runtime validation failed: {source}"))]
    Runtime {
        /// The underlying runtime error.
        source: crate::strategy_executor::ExecutorError,
    },

    /// Trace storage lookup failed.
    #[snafu(display("trace error: {source}"))]
    Trace {
        /// The underlying trace error.
        source: crate::trace::TraceError,
    },

    /// Filesystem I/O failed.
    #[snafu(display("I/O error: {source}"))]
    Io {
        /// The underlying I/O error.
        source: std::io::Error,
    },

    /// JSON serialization failed.
    #[snafu(display("serialization error: {source}"))]
    Serialize {
        /// The underlying serde error.
        source: serde_json::Error,
    },
}

/// Module-level result alias.
pub type Result<T> = std::result::Result<T, PromoterError>;

/// Metadata persisted alongside a promoted WASM strategy binary.
#[derive(Debug, Clone, Serialize, Deserialize, Builder)]
pub struct PromotedStrategy {
    /// The experiment that produced this strategy.
    experiment_id: Uuid,
    /// The hypothesis the experiment tested.
    hypothesis_id: Uuid,
    /// Filesystem path to the promoted WASM binary.
    wasm_path:     PathBuf,
    /// Strategy metadata extracted from the WASM module.
    meta:          StrategyMeta,
    /// Filesystem path to the saved `.rs` source code, if available.
    source_path:   Option<PathBuf>,
}

impl PromotedStrategy {
    /// Returns the experiment ID.
    pub const fn experiment_id(&self) -> Uuid { self.experiment_id }

    /// Returns the hypothesis ID.
    pub const fn hypothesis_id(&self) -> Uuid { self.hypothesis_id }

    /// Returns the path to the promoted WASM binary.
    pub fn wasm_path(&self) -> &Path { &self.wasm_path }

    /// Returns the strategy metadata.
    pub const fn meta(&self) -> &StrategyMeta { &self.meta }

    /// Returns the path to the saved `.rs` source code, if available.
    pub fn source_path(&self) -> Option<&Path> { self.source_path.as_deref() }
}

/// Handles promotion of candidate strategies from research to paper trading.
///
/// When a research experiment is accepted, the promoter recompiles its source
/// code to WASM, validates the module, and saves both the binary and metadata
/// to `strategies/promoted/` for downstream consumption.
#[derive(Builder)]
pub struct StrategyPromoter {
    /// Trace storage for looking up experiments and hypotheses.
    trace:        Trace,
    /// WASM runtime for validating promoted modules.
    runtime:      WasmExecutor,
    /// Strategy compiler for producing WASM from source code.
    compiler:     StrategyCompiler,
    /// Base directory for promoted strategies (e.g. `strategies/promoted/`).
    promoted_dir: PathBuf,
}

impl StrategyPromoter {
    /// Promote a candidate strategy by experiment ID.
    ///
    /// Looks up the experiment in the trace, recompiles its code to WASM,
    /// validates the module, and saves the binary + metadata to the promoted
    /// directory. Returns metadata about the promoted strategy.
    pub async fn promote(&self, experiment_id: Uuid) -> Result<PromotedStrategy> {
        // 1. Look up experiment from trace
        let experiment = self
            .trace
            .get_experiment(experiment_id)
            .context(TraceSnafu)?
            .ok_or(PromoterError::ExperimentNotFound { id: experiment_id })?;

        let hypothesis_id = experiment.hypothesis_id;

        // 2. Compile strategy code to WASM
        let compile_result = self
            .compiler
            .compile(&experiment.strategy_code)
            .await
            .context(CompileSnafu)?;

        if !compile_result.success {
            return Err(PromoterError::CompileErrors {
                errors: compile_result.errors,
            });
        }

        let wasm_bytes = compile_result
            .wasm_bytes
            .expect("success implies wasm_bytes");

        // 3. Load into runtime to validate and extract metadata
        let mut loaded = self.runtime.load(&wasm_bytes).context(RuntimeSnafu)?;
        let meta = loaded.meta().context(RuntimeSnafu)?;

        // 4. Ensure promoted directory exists
        std::fs::create_dir_all(&self.promoted_dir).context(IoSnafu)?;

        // 5. Save WASM binary
        let wasm_path = self.promoted_dir.join(format!("{experiment_id}.wasm"));
        std::fs::write(&wasm_path, &wasm_bytes).context(IoSnafu)?;

        // 6. Save strategy source code alongside the binary
        let source_path = self.promoted_dir.join(format!("{experiment_id}.rs"));
        std::fs::write(&source_path, &experiment.strategy_code).context(IoSnafu)?;

        // 7. Build and save metadata
        let promoted = PromotedStrategy::builder()
            .experiment_id(experiment_id)
            .hypothesis_id(hypothesis_id)
            .wasm_path(wasm_path)
            .source_path(source_path)
            .meta(meta)
            .build();

        let meta_path = self.promoted_dir.join(format!("{experiment_id}.json"));
        let meta_json = serde_json::to_string_pretty(&promoted).context(SerializeSnafu)?;
        std::fs::write(&meta_path, meta_json).context(IoSnafu)?;

        Ok(promoted)
    }

    /// Promote a strategy directly from WASM bytes, skipping recompilation.
    ///
    /// Used when WASM bytes are already available (e.g. immediately after
    /// compilation in the research loop) to avoid a redundant compile step.
    /// When `source_code` is provided, the `.rs` source is saved alongside the
    /// binary.
    pub fn promote_from_wasm(
        &self,
        experiment_id: Uuid,
        hypothesis_id: Uuid,
        wasm_bytes: &[u8],
        source_code: Option<&str>,
    ) -> Result<PromotedStrategy> {
        // 1. Load into runtime to validate and extract metadata
        let mut loaded = self.runtime.load(wasm_bytes).context(RuntimeSnafu)?;
        let meta = loaded.meta().context(RuntimeSnafu)?;

        // 2. Ensure promoted directory exists
        std::fs::create_dir_all(&self.promoted_dir).context(IoSnafu)?;

        // 3. Save WASM binary
        let wasm_path = self.promoted_dir.join(format!("{experiment_id}.wasm"));
        std::fs::write(&wasm_path, wasm_bytes).context(IoSnafu)?;

        // 4. Save strategy source code if provided
        let source_path = if let Some(code) = source_code {
            let path = self.promoted_dir.join(format!("{experiment_id}.rs"));
            std::fs::write(&path, code).context(IoSnafu)?;
            Some(path)
        } else {
            None
        };

        // 5. Build and save metadata
        let promoted = PromotedStrategy::builder()
            .experiment_id(experiment_id)
            .hypothesis_id(hypothesis_id)
            .wasm_path(wasm_path)
            .maybe_source_path(source_path)
            .meta(meta)
            .build();

        let meta_path = self.promoted_dir.join(format!("{experiment_id}.json"));
        let meta_json = serde_json::to_string_pretty(&promoted).context(SerializeSnafu)?;
        std::fs::write(&meta_path, meta_json).context(IoSnafu)?;

        Ok(promoted)
    }

    /// List all promoted strategies by reading metadata files from the promoted
    /// directory.
    pub fn list_promoted(&self) -> Result<Vec<PromotedStrategy>> {
        if !self.promoted_dir.exists() {
            return Ok(vec![]);
        }

        let mut promoted = Vec::new();
        let entries = std::fs::read_dir(&self.promoted_dir).context(IoSnafu)?;

        for entry in entries {
            let entry = entry.context(IoSnafu)?;
            let path = entry.path();

            if path.extension().is_some_and(|ext| ext == "json") {
                let contents = std::fs::read_to_string(&path).context(IoSnafu)?;
                let strategy: PromotedStrategy =
                    serde_json::from_str(&contents).context(SerializeSnafu)?;
                promoted.push(strategy);
            }
        }

        Ok(promoted)
    }

    /// Load a promoted strategy's WASM bytes by experiment ID.
    pub fn load_promoted_wasm(&self, experiment_id: Uuid) -> Result<Vec<u8>> {
        let wasm_path = self.promoted_dir.join(format!("{experiment_id}.wasm"));
        std::fs::read(&wasm_path).context(IoSnafu)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_promoted_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        let promoted_dir = dir.path().join("promoted");

        let sut = StrategyPromoter::builder()
            .trace(Trace::open(&dir.path().join("trace")).unwrap())
            .runtime(WasmExecutor::builder().build())
            .compiler(
                StrategyCompiler::builder()
                    .template_dir(PathBuf::from("nonexistent"))
                    .build(),
            )
            .promoted_dir(promoted_dir)
            .build();

        let result = sut.list_promoted().unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn list_promoted_reads_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let promoted_dir = dir.path().join("promoted");
        std::fs::create_dir_all(&promoted_dir).unwrap();

        let exp_id = Uuid::new_v4();
        let hyp_id = Uuid::new_v4();
        let meta = StrategyMeta {
            name:        "test-strategy".into(),
            version:     1,
            api_version: rara_strategy_api::API_VERSION,
            description: "A test".into(),
        };

        let strategy = PromotedStrategy::builder()
            .experiment_id(exp_id)
            .hypothesis_id(hyp_id)
            .wasm_path(promoted_dir.join(format!("{exp_id}.wasm")))
            .meta(meta)
            .build();

        let json = serde_json::to_string_pretty(&strategy).unwrap();
        std::fs::write(promoted_dir.join(format!("{exp_id}.json")), json).unwrap();

        let sut = StrategyPromoter::builder()
            .trace(Trace::open(&dir.path().join("trace")).unwrap())
            .runtime(WasmExecutor::builder().build())
            .compiler(
                StrategyCompiler::builder()
                    .template_dir(PathBuf::from("nonexistent"))
                    .build(),
            )
            .promoted_dir(promoted_dir)
            .build();

        let result = sut.list_promoted().unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].experiment_id, exp_id);
        assert_eq!(result[0].hypothesis_id, hyp_id);
        assert_eq!(result[0].meta().name, "test-strategy");
    }
}
