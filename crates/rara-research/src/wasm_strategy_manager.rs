//! WASM-based strategy manager implementation.
//!
//! Composes [`StrategyStore`], [`StrategyCoder`], [`StrategyCompiler`],
//! and [`WasmExecutor`] to provide the full strategy lifecycle for
//! Rust-to-WASM compiled strategies.

use async_trait::async_trait;
use bon::Builder;
use uuid::Uuid;

use rara_domain::research::{Hypothesis, ResearchStrategy, ResearchStrategyStatus};

use crate::compiler::StrategyCompiler;
use crate::strategy_coder::StrategyCoder;
use crate::strategy_executor::{StrategyExecutor, StrategyHandle};
use crate::strategy_manager::{Result, StrategyManager, StrategyManagerError};
use crate::strategy_store::StrategyStore;
use crate::wasm_executor::WasmExecutor;

/// WASM-based strategy manager.
///
/// Handles the full lifecycle of Rust → WASM strategies:
/// code generation via LLM, compilation via cargo, artifact storage,
/// and runtime loading via wasmtime.
#[derive(Builder)]
pub struct WasmStrategyManager {
    /// Sled-backed strategy persistence.
    store: StrategyStore,
    /// LLM-backed code generator.
    coder: StrategyCoder,
    /// Rust → WASM compiler.
    compiler: StrategyCompiler,
    /// WASM runtime for loading artifacts.
    executor: WasmExecutor,
}

#[async_trait]
impl StrategyManager for WasmStrategyManager {
    async fn generate_code(&self, hypothesis: &Hypothesis, context: &str) -> Result<String> {
        self.coder
            .generate_code(hypothesis, context)
            .await
            .map_err(|e| StrategyManagerError::CodeGen {
                message: e.to_string(),
            })
    }

    async fn try_compile(&self, source_code: &str) -> Result<Vec<u8>> {
        let result = self
            .compiler
            .compile(source_code)
            .await
            .map_err(|e| StrategyManagerError::Compile {
                message: e.to_string(),
            })?;

        if !result.success {
            return Err(StrategyManagerError::CompileFailed {
                errors: result.errors,
            });
        }

        Ok(result.wasm_bytes.expect("success implies wasm_bytes"))
    }

    fn save_strategy(
        &self,
        hypothesis_id: Uuid,
        source_code: &str,
        artifact: &[u8],
    ) -> Result<ResearchStrategy> {
        let strategy = ResearchStrategy::builder()
            .hypothesis_id(hypothesis_id)
            .source_code(source_code)
            .build();

        self.store
            .save(&strategy)
            .map_err(|e| StrategyManagerError::Store {
                message: e.to_string(),
            })?;

        self.store
            .save_artifact(strategy.id, artifact)
            .map_err(|e| StrategyManagerError::Store {
                message: e.to_string(),
            })?;

        Ok(strategy)
    }

    async fn fix_errors(
        &self,
        source_code: &str,
        errors: &[String],
        hypothesis: &Hypothesis,
    ) -> Result<String> {
        self.coder
            .fix_errors(source_code, errors, hypothesis)
            .await
            .map_err(|e| StrategyManagerError::CodeGen {
                message: e.to_string(),
            })
    }

    fn load_handle(&self, strategy_id: Uuid) -> Result<Box<dyn StrategyHandle>> {
        let artifact = self
            .store
            .load_artifact(strategy_id)
            .map_err(|e| StrategyManagerError::Store {
                message: e.to_string(),
            })?;

        self.executor
            .load(&artifact)
            .map_err(|e| StrategyManagerError::Load {
                message: e.to_string(),
            })
    }

    fn update_status(&self, strategy_id: Uuid, status: ResearchStrategyStatus) -> Result<()> {
        self.store
            .update_status(strategy_id, status)
            .map_err(|e| StrategyManagerError::Store {
                message: e.to_string(),
            })
    }
}
