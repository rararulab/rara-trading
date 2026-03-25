//! Generated strategy WASM module.
//!
//! The section between STRATEGY_IMPL markers is replaced by LLM-generated code.
//! WASM export functions below are fixed and handle JSON serialization.

use rara_strategy_api::{Candle, RiskLevels, Side, Signal, StrategyMeta, API_VERSION};

// ===== STRATEGY_IMPL START =====

fn meta() -> StrategyMeta {
    StrategyMeta {
        name: "placeholder".into(),
        version: 1,
        api_version: API_VERSION,
        description: "Placeholder strategy".into(),
    }
}

fn on_candles(candles: &[Candle]) -> Signal {
    let _ = candles;
    Signal::Hold
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

// ===== STRATEGY_IMPL END =====

// ===== WASM EXPORTS (fixed, do not modify) =====

use std::cell::UnsafeCell;

/// Single-threaded global buffer. Sound in WASM where there is only one thread.
struct WasmBuf(UnsafeCell<Vec<u8>>);

// SAFETY: WASM modules are single-threaded; no concurrent access is possible.
unsafe impl Sync for WasmBuf {}

impl WasmBuf {
    const fn new() -> Self {
        Self(UnsafeCell::new(Vec::new()))
    }

    /// Return a raw pointer to the inner `Vec` for mutation.
    #[expect(clippy::mut_from_ref)]
    fn get_mut(&self) -> &mut Vec<u8> {
        // SAFETY: WASM is single-threaded; only one caller at a time.
        unsafe { &mut *self.0.get() }
    }

    /// Return a shared reference to the inner `Vec` for reading.
    fn get(&self) -> &Vec<u8> {
        // SAFETY: WASM is single-threaded; no concurrent mutation.
        unsafe { &*self.0.get() }
    }
}

static INPUT_BUF: WasmBuf = WasmBuf::new();
static OUTPUT_BUF: WasmBuf = WasmBuf::new();

/// Allocate memory for host to write input data.
#[unsafe(no_mangle)]
pub extern "C" fn alloc(len: u32) -> *mut u8 {
    let buf = INPUT_BUF.get_mut();
    *buf = Vec::with_capacity(len as usize);
    // SAFETY: we just allocated this capacity; host will fill the bytes.
    unsafe { buf.set_len(len as usize) };
    buf.as_mut_ptr()
}

/// Get pointer to output buffer.
#[unsafe(no_mangle)]
pub extern "C" fn get_output_ptr() -> *const u8 {
    OUTPUT_BUF.get().as_ptr()
}

/// Get length of output buffer.
#[unsafe(no_mangle)]
pub extern "C" fn get_output_len() -> u32 {
    OUTPUT_BUF.get().len() as u32
}

/// Return strategy metadata as JSON.
#[unsafe(no_mangle)]
pub extern "C" fn wasm_meta() -> u32 {
    let m = meta();
    let json = serde_json::to_vec(&m).unwrap_or_default();
    let buf = OUTPUT_BUF.get_mut();
    *buf = json;
    buf.len() as u32
}

/// Process candles and return signal as JSON.
#[unsafe(no_mangle)]
pub extern "C" fn wasm_on_candles() -> u32 {
    let candles: Vec<Candle> = serde_json::from_slice(INPUT_BUF.get()).unwrap_or_default();
    let signal = on_candles(&candles);
    let json = serde_json::to_vec(&signal).unwrap_or_default();
    let buf = OUTPUT_BUF.get_mut();
    *buf = json;
    buf.len() as u32
}

/// Compute risk levels from JSON input {entry_price, side}.
#[unsafe(no_mangle)]
pub extern "C" fn wasm_risk_levels() -> u32 {
    #[derive(serde::Deserialize)]
    struct Input {
        entry_price: f64,
        side: Side,
    }
    let input: Input = serde_json::from_slice(INPUT_BUF.get()).unwrap_or(Input {
        entry_price: 0.0,
        side: Side::Long,
    });
    let levels = risk_levels(input.entry_price, input.side);
    let json = serde_json::to_vec(&levels).unwrap_or_default();
    let buf = OUTPUT_BUF.get_mut();
    *buf = json;
    buf.len() as u32
}
