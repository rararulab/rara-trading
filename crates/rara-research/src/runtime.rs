//! WASM strategy runtime — loads compiled `.wasm` strategy files and calls
//! their exported functions via the JSON-based communication protocol.

use std::path::PathBuf;

use bon::Builder;
use rara_strategy_api::{Candle, RiskLevels, Side, Signal, StrategyMeta};
use snafu::{ResultExt, Snafu};
use wasmtime::{Engine, Linker, Memory, Module, Store, TypedFunc};
use wasmtime_wasi::preview1::WasiP1Ctx;

/// Errors from WASM strategy runtime.
#[derive(Debug, Snafu)]
#[snafu(visibility(pub))]
pub enum RuntimeError {
    /// Failed to create WASM engine or module.
    #[snafu(display("WASM engine error: {source}"))]
    WasmEngine { source: wasmtime::Error },

    /// Failed to call WASM function.
    #[snafu(display("WASM call failed: {source}"))]
    WasmCall { source: wasmtime::Error },

    /// Failed to deserialize WASM output.
    #[snafu(display("failed to parse WASM output: {source}"))]
    Deserialize { source: serde_json::Error },

    /// Failed to serialize input for WASM.
    #[snafu(display("failed to serialize WASM input: {source}"))]
    Serialize { source: serde_json::Error },

    /// WASM module missing required export.
    #[snafu(display("missing WASM export: {name}"))]
    MissingExport { name: String },

    /// Memory access error.
    #[snafu(display("WASM memory error: {message}"))]
    MemoryAccess { message: String },
}

/// Module-level result alias.
pub type Result<T> = std::result::Result<T, RuntimeError>;

/// Runtime for loading and executing WASM strategies.
#[derive(Debug, Builder)]
pub struct StrategyRuntime {
    /// Maximum fuel (computation budget) for WASM execution.
    #[builder(default = 10_000_000)]
    fuel_limit: u64,
}

/// A loaded WASM strategy ready to execute.
pub struct LoadedStrategy {
    store: Store<WasiP1Ctx>,
    memory: Memory,
    // Cached typed function handles
    fn_alloc: TypedFunc<u32, u32>,
    fn_get_output_ptr: TypedFunc<(), u32>,
    fn_get_output_len: TypedFunc<(), u32>,
    fn_wasm_meta: TypedFunc<(), u32>,
    fn_wasm_on_candles: TypedFunc<(), u32>,
    fn_wasm_risk_levels: TypedFunc<(), u32>,
}

impl StrategyRuntime {
    /// Load a compiled WASM strategy from raw bytes.
    pub fn load(&self, wasm_bytes: &[u8]) -> Result<LoadedStrategy> {
        let mut config = wasmtime::Config::new();
        config.consume_fuel(true);
        let engine = Engine::new(&config).context(WasmEngineSnafu)?;

        let module = Module::new(&engine, wasm_bytes).context(WasmEngineSnafu)?;

        // Build a minimal WASI context for wasm32-wasip1 modules
        let wasi_ctx = wasmtime_wasi::WasiCtxBuilder::new().build_p1();

        let mut store = Store::new(&engine, wasi_ctx);
        store.set_fuel(self.fuel_limit).context(WasmEngineSnafu)?;

        let mut linker = Linker::new(&engine);
        wasmtime_wasi::preview1::add_to_linker_sync(&mut linker, |ctx| ctx)
            .context(WasmEngineSnafu)?;

        let instance = linker
            .instantiate(&mut store, &module)
            .context(WasmEngineSnafu)?;

        let memory = instance
            .get_memory(&mut store, "memory")
            .ok_or(RuntimeError::MissingExport {
                name: "memory".into(),
            })?;

        let fn_alloc = instance
            .get_typed_func::<u32, u32>(&mut store, "alloc")
            .context(WasmEngineSnafu)?;
        let fn_get_output_ptr = instance
            .get_typed_func::<(), u32>(&mut store, "get_output_ptr")
            .context(WasmEngineSnafu)?;
        let fn_get_output_len = instance
            .get_typed_func::<(), u32>(&mut store, "get_output_len")
            .context(WasmEngineSnafu)?;
        let fn_wasm_meta = instance
            .get_typed_func::<(), u32>(&mut store, "wasm_meta")
            .context(WasmEngineSnafu)?;
        let fn_wasm_on_candles = instance
            .get_typed_func::<(), u32>(&mut store, "wasm_on_candles")
            .context(WasmEngineSnafu)?;
        let fn_wasm_risk_levels = instance
            .get_typed_func::<(), u32>(&mut store, "wasm_risk_levels")
            .context(WasmEngineSnafu)?;

        Ok(LoadedStrategy {
            store,
            memory,
            fn_alloc,
            fn_get_output_ptr,
            fn_get_output_len,
            fn_wasm_meta,
            fn_wasm_on_candles,
            fn_wasm_risk_levels,
        })
    }

    /// Load a compiled WASM strategy from a file path.
    pub fn load_file(&self, path: &PathBuf) -> Result<LoadedStrategy> {
        let bytes = std::fs::read(path).map_err(|e| RuntimeError::MemoryAccess {
            message: format!("failed to read WASM file: {e}"),
        })?;
        self.load(&bytes)
    }
}

impl LoadedStrategy {
    /// Get strategy metadata.
    pub fn meta(&mut self) -> Result<StrategyMeta> {
        self.fn_wasm_meta
            .call(&mut self.store, ())
            .context(WasmCallSnafu)?;
        let output = self.read_output()?;
        serde_json::from_slice(&output).context(DeserializeSnafu)
    }

    /// Process candles and get a trading signal.
    pub fn on_candles(&mut self, candles: &[Candle]) -> Result<Signal> {
        let input = serde_json::to_vec(candles).context(SerializeSnafu)?;
        self.write_input(&input)?;
        self.fn_wasm_on_candles
            .call(&mut self.store, ())
            .context(WasmCallSnafu)?;
        let output = self.read_output()?;
        serde_json::from_slice(&output).context(DeserializeSnafu)
    }

    /// Get risk levels for a position.
    pub fn risk_levels(&mut self, entry_price: f64, side: Side) -> Result<RiskLevels> {
        #[derive(serde::Serialize)]
        struct Input {
            entry_price: f64,
            side: Side,
        }
        let input =
            serde_json::to_vec(&Input { entry_price, side }).context(SerializeSnafu)?;
        self.write_input(&input)?;
        self.fn_wasm_risk_levels
            .call(&mut self.store, ())
            .context(WasmCallSnafu)?;
        let output = self.read_output()?;
        serde_json::from_slice(&output).context(DeserializeSnafu)
    }

    /// Write JSON bytes into WASM input buffer via `alloc()`.
    fn write_input(&mut self, data: &[u8]) -> Result<()> {
        let len: u32 = u32::try_from(data.len()).map_err(|_| RuntimeError::MemoryAccess {
            message: format!("input too large: {} bytes", data.len()),
        })?;
        let ptr = self
            .fn_alloc
            .call(&mut self.store, len)
            .context(WasmCallSnafu)?;
        let start = ptr as usize;
        let end = start + data.len();
        let mem = self.memory.data_mut(&mut self.store);
        if end > mem.len() {
            return Err(RuntimeError::MemoryAccess {
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
            .context(WasmCallSnafu)? as usize;
        let len = self
            .fn_get_output_len
            .call(&mut self.store, ())
            .context(WasmCallSnafu)? as usize;
        let mem = self.memory.data(&self.store);
        let end = ptr + len;
        if end > mem.len() {
            return Err(RuntimeError::MemoryAccess {
                message: format!(
                    "read out of bounds: offset {ptr}..{end}, memory size {}",
                    mem.len()
                ),
            });
        }
        Ok(mem[ptr..end].to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiler::StrategyCompiler;

    fn template_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../strategies/template")
    }

    #[tokio::test]
    async fn loads_and_calls_wasm_strategy() {
        let compiler = StrategyCompiler::builder()
            .template_dir(template_dir())
            .build();

        let code = r#"
fn meta() -> StrategyMeta {
    StrategyMeta {
        name: "test-strategy".into(),
        version: 1,
        api_version: API_VERSION,
        description: "A test strategy".into(),
    }
}

fn on_candles(candles: &[Candle]) -> Signal {
    if candles.last().map_or(false, |c| c.close > c.open) {
        Signal::Entry {
            side: Side::Long,
            strength: 0.7,
        }
    } else {
        Signal::Hold
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
        assert!(
            result.success,
            "Compilation failed: {:?}",
            result.errors
        );
        let wasm_bytes = result.wasm_bytes.expect("expected wasm bytes");

        let runtime = StrategyRuntime::builder().build();
        let mut strategy = runtime.load(&wasm_bytes).expect("should load wasm");

        // Test meta
        let meta = strategy.meta().expect("meta() should succeed");
        assert_eq!(meta.name, "test-strategy");
        assert_eq!(meta.api_version, rara_strategy_api::API_VERSION);

        // Test on_candles with bullish candle (close > open)
        let candles = vec![Candle {
            timestamp: 1,
            open: 100.0,
            high: 105.0,
            low: 99.0,
            close: 103.0,
            volume: 1000.0,
        }];
        let signal = strategy.on_candles(&candles).expect("on_candles should succeed");
        assert!(matches!(signal, Signal::Entry { side: Side::Long, .. }));

        // Test on_candles with bearish candle (close < open)
        let bearish = vec![Candle {
            timestamp: 2,
            open: 105.0,
            high: 106.0,
            low: 99.0,
            close: 100.0,
            volume: 1000.0,
        }];
        let signal = strategy
            .on_candles(&bearish)
            .expect("on_candles should succeed");
        assert!(matches!(signal, Signal::Hold));

        // Test risk_levels
        let levels = strategy
            .risk_levels(100.0, Side::Long)
            .expect("risk_levels should succeed");
        assert!(
            (levels.stop_loss - 98.0).abs() < 0.01,
            "stop_loss: {}",
            levels.stop_loss
        );
        assert!(
            (levels.take_profit - 102.0).abs() < 0.01,
            "take_profit: {}",
            levels.take_profit
        );
    }
}
