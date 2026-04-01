//! WASM-based strategy executor — implements [`StrategyExecutor`] and
//! [`StrategyHandle`] by delegating to `wasmtime` for compiled `.wasm` strategy
//! files.

use bon::Builder;
use strategy_api::{Candle, StrategyMeta, StrategyOutput};
use wasmtime::{Engine, Linker, Memory, Module, Store, TypedFunc};
use wasmtime_wasi::p1::WasiP1Ctx;

use crate::strategy_executor::{ExecutorError, Result, StrategyExecutor, StrategyHandle};

/// WASM-based strategy executor powered by `wasmtime`.
///
/// Loads compiled `.wasm` strategy artifacts and produces
/// [`WasmStrategyHandle`] instances that communicate via the JSON-based
/// protocol.
#[derive(Debug, Builder)]
pub struct WasmExecutor {
    /// Maximum fuel (computation budget) for WASM execution.
    #[builder(default = 10_000_000)]
    pub fuel_limit: u64,
}

/// A loaded WASM strategy ready to execute.
///
/// Wraps a `wasmtime` store and cached function handles for the strategy
/// protocol. v2 API exports only `wasm_meta()` and `wasm_on_candles()`.
pub struct WasmStrategyHandle {
    store:              Store<WasiP1Ctx>,
    memory:             Memory,
    // Cached typed function handles
    fn_alloc:           TypedFunc<u32, u32>,
    fn_get_output_ptr:  TypedFunc<(), u32>,
    fn_get_output_len:  TypedFunc<(), u32>,
    fn_wasm_meta:       TypedFunc<(), u32>,
    fn_wasm_on_candles: TypedFunc<(), u32>,
}

/// Map a wasmtime error to an `ExecutorError::Load`.
#[allow(clippy::needless_pass_by_value)]
fn load_err(context: &str, source: wasmtime::Error) -> ExecutorError {
    ExecutorError::Load {
        message: format!("{context}: {source}"),
    }
}

/// Map a wasmtime error to an `ExecutorError::Execution`.
#[allow(clippy::needless_pass_by_value)]
fn exec_err(context: &str, source: wasmtime::Error) -> ExecutorError {
    ExecutorError::Execution {
        message: format!("{context}: {source}"),
    }
}

/// Map a `serde_json` error to an `ExecutorError::Execution`.
#[allow(clippy::needless_pass_by_value)]
fn serde_err(context: &str, source: serde_json::Error) -> ExecutorError {
    ExecutorError::Execution {
        message: format!("{context}: {source}"),
    }
}

impl StrategyExecutor for WasmExecutor {
    /// Load a compiled WASM strategy from raw bytes.
    fn load(&self, artifact: &[u8]) -> Result<Box<dyn StrategyHandle>> {
        let mut config = wasmtime::Config::new();
        config.consume_fuel(true);
        let engine = Engine::new(&config).map_err(|e| load_err("WASM engine error", e))?;

        let module =
            Module::new(&engine, artifact).map_err(|e| load_err("WASM module error", e))?;

        let wasi_ctx = wasmtime_wasi::WasiCtxBuilder::new().build_p1();

        let mut store = Store::new(&engine, wasi_ctx);
        store
            .set_fuel(self.fuel_limit)
            .map_err(|e| load_err("failed to set fuel", e))?;

        let mut linker = Linker::new(&engine);
        wasmtime_wasi::p1::add_to_linker_sync(&mut linker, |ctx| ctx)
            .map_err(|e| load_err("WASI linker error", e))?;

        let instance = linker
            .instantiate(&mut store, &module)
            .map_err(|e| load_err("WASM instantiation error", e))?;

        let memory =
            instance
                .get_memory(&mut store, "memory")
                .ok_or_else(|| ExecutorError::Load {
                    message: "missing WASM export: memory".into(),
                })?;

        let fn_alloc: TypedFunc<u32, u32> = instance
            .get_typed_func(&mut store, "alloc")
            .map_err(|e| load_err("missing export: alloc", e))?;
        let fn_get_output_ptr: TypedFunc<(), u32> = instance
            .get_typed_func(&mut store, "get_output_ptr")
            .map_err(|e| load_err("missing export: get_output_ptr", e))?;
        let fn_get_output_len: TypedFunc<(), u32> = instance
            .get_typed_func(&mut store, "get_output_len")
            .map_err(|e| load_err("missing export: get_output_len", e))?;
        let fn_wasm_meta: TypedFunc<(), u32> = instance
            .get_typed_func(&mut store, "wasm_meta")
            .map_err(|e| load_err("missing export: wasm_meta", e))?;
        let fn_wasm_on_candles: TypedFunc<(), u32> = instance
            .get_typed_func(&mut store, "wasm_on_candles")
            .map_err(|e| load_err("missing export: wasm_on_candles", e))?;

        Ok(Box::new(WasmStrategyHandle {
            store,
            memory,
            fn_alloc,
            fn_get_output_ptr,
            fn_get_output_len,
            fn_wasm_meta,
            fn_wasm_on_candles,
        }))
    }
}

impl StrategyHandle for WasmStrategyHandle {
    /// Return strategy metadata.
    fn meta(&mut self) -> Result<StrategyMeta> {
        self.fn_wasm_meta
            .call(&mut self.store, ())
            .map_err(|e| exec_err("wasm_meta call failed", e))?;
        let output = self.read_output()?;
        serde_json::from_slice(&output).map_err(|e| serde_err("failed to deserialize meta", e))
    }

    /// Process candle history and return strategy output.
    fn on_candles(&mut self, candles: &[Candle]) -> Result<StrategyOutput> {
        let input =
            serde_json::to_vec(candles).map_err(|e| serde_err("failed to serialize candles", e))?;
        self.write_input(&input)?;
        self.fn_wasm_on_candles
            .call(&mut self.store, ())
            .map_err(|e| exec_err("wasm_on_candles call failed", e))?;
        let output = self.read_output()?;
        serde_json::from_slice(&output)
            .map_err(|e| serde_err("failed to deserialize on_candles output", e))
    }
}

impl WasmStrategyHandle {
    /// Write JSON bytes into WASM input buffer via `alloc()`.
    fn write_input(&mut self, data: &[u8]) -> Result<()> {
        let len: u32 = u32::try_from(data.len()).map_err(|_| ExecutorError::Execution {
            message: format!("input too large: {} bytes", data.len()),
        })?;
        let ptr = self
            .fn_alloc
            .call(&mut self.store, len)
            .map_err(|e| exec_err("alloc call failed", e))?;
        let start = ptr as usize;
        let end = start + data.len();
        let mem = self.memory.data_mut(&mut self.store);
        if end > mem.len() {
            return Err(ExecutorError::Execution {
                message: format!(
                    "write out of bounds: offset {start}..{end}, memory size {}",
                    mem.len()
                ),
            });
        }
        mem[start..end].copy_from_slice(data);
        Ok(())
    }

    /// Read JSON bytes from WASM output buffer.
    fn read_output(&mut self) -> Result<Vec<u8>> {
        let ptr = self
            .fn_get_output_ptr
            .call(&mut self.store, ())
            .map_err(|e| exec_err("get_output_ptr call failed", e))? as usize;
        let len = self
            .fn_get_output_len
            .call(&mut self.store, ())
            .map_err(|e| exec_err("get_output_len call failed", e))? as usize;
        let mem = self.memory.data(&self.store);
        let end = ptr + len;
        if end > mem.len() {
            return Err(ExecutorError::Execution {
                message: format!(
                    "read out of bounds: offset {ptr}..{end}, memory size {}",
                    mem.len()
                ),
            });
        }
        Ok(mem[ptr..end].to_vec())
    }
}
