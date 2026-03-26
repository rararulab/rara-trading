# Architecture

This document describes the technical architecture of rara-trading: crate responsibilities, data flow, and key design decisions.

## Crate Dependency Graph

```
                    rara-trading (CLI binary)
                    ├── rara-research
                    │   ├── rara-domain
                    │   ├── rara-event-bus
                    │   ├── rara-infra
                    │   ├── rara-market-data
                    │   ├── rara-strategy-api
                    │   └── wasmtime
                    ├── rara-trading-engine
                    │   ├── rara-domain
                    │   ├── rara-event-bus
                    │   └── rara-infra
                    ├── rara-feedback
                    │   ├── rara-domain
                    │   └── rara-event-bus
                    ├── rara-sentinel
                    │   ├── rara-domain
                    │   └── rara-event-bus
                    ├── rara-server (gRPC)
                    │   └── rara-event-bus
                    ├── rara-tui (TUI client)
                    │   └── rara-server (proto types)
                    ├── rara-agent
                    └── rara-infra
```

## Crate Responsibilities

### rara-domain

Core domain models shared across all components. No I/O, no side effects.

- `Hypothesis` — a testable market hypothesis with parent lineage
- `Experiment` — a single research iteration (hypothesis + code + backtest result)
- `Contract` — trading pair abstraction (e.g. BTC-USDT)
- `BacktestResult` — PnL, Sharpe, drawdown, win rate, trade count
- `ResearchStrategy` — strategy lifecycle (Compiled → Accepted → Promoted → Archived)

### rara-event-bus

Sled-backed persistent event bus. Components publish and subscribe to events asynchronously. Each subscriber tracks its own read cursor, enabling independent consumption rates.

Key event topics: `research.*`, `trading.*`, `feedback.*`, `sentinel.*`

### rara-research

The autonomous research engine. Contains the most complex subsystems:

| Module | Purpose |
|--------|---------|
| `research_loop` | Orchestrates the hypothesis → code → compile → backtest → evaluate cycle |
| `hypothesis_gen` | LLM-driven hypothesis generation from market data and past results |
| `strategy_coder` | LLM generates Rust strategy code from a hypothesis |
| `compiler` | Compiles generated Rust code to `wasm32-wasip1` using a template scaffold |
| `wasm_executor` | Loads and runs WASM strategies via wasmtime with fuel limits |
| `backtester` | Evaluates strategies against historical data using barter-rs |
| `backtest_pool` | Parallel multi-timeframe backtesting |
| `strategy_store` | Sled + filesystem persistence for strategy artifacts |
| `strategy_promoter` | Saves accepted strategies to the promoted directory |
| `strategy_registry` | Fetches pre-built WASM strategies from GitHub Releases |
| `trace` | Persistent experiment history for analysis and lineage tracking |

### rara-strategy-api

Minimal crate (~73 lines) that compiles to both native and `wasm32-wasip1`. Defines the contract between the host runtime and WASM strategies:

```rust
pub const API_VERSION: u32 = 1;

pub struct Candle { timestamp, open, high, low, close, volume }
pub enum Signal { Entry { side, strength }, Exit, Hold }
pub struct RiskLevels { stop_loss, take_profit }
pub struct StrategyMeta { name, version, api_version, description }
```

Communication uses JSON serialization through WASM linear memory. The host allocates input buffers, the strategy writes output buffers.

### rara-market-data

TimescaleDB-backed market data storage with smart fetching:

- **Binance fetcher** — REST API for historical 1m klines, auto-pagination
- **Yahoo fetcher** — Daily OHLCV via yahoo-finance API
- **Smart fetch** — skips already-stored date ranges, only fetches gaps
- **Store** — TimescaleDB hypertable with `(exchange, symbol, interval, ts)` primary key

### rara-trading-engine

Order execution with risk controls:

- **Guard pipeline** — configurable chain of pre-trade checks (max position, drawdown limit, sentinel gate)
- **PaperBroker** — simulated execution with realistic fills
- **CcxtBroker** — real exchange execution via ccxt-rust

### rara-feedback

Strategy evaluation and lifecycle management:

- Aggregates paper trading metrics (Sharpe, drawdown, win rate)
- Makes promote/hold/demote decisions based on configurable thresholds
- Publishes lifecycle events to the event bus

### rara-sentinel

External signal monitoring for risk management:

- RSS feed polling for market-moving news
- Webhook receiver for external alerts
- LLM-based signal classification (Critical / Warning / Info)
- Critical signals block all trading via the event bus

### rara-agent

LLM backend abstraction:

- Supports Claude (Anthropic API) and Codex (OpenAI) backends
- CLI executor with timeout and streaming support
- Configured via `config.toml`

### rara-server

gRPC server providing real-time system access:

```protobuf
service RaraService {
    rpc GetSystemStatus(Empty) returns (SystemStatus);
    rpc StreamEvents(StreamEventsRequest) returns (stream Event);
}
```

- `SystemStatus` — connection states, strategy count, uptime
- `StreamEvents` — real-time event stream with optional topic filtering

### rara-tui

Terminal dashboard built with ratatui + crossterm:

- **4 tabs**: Overview, Research, Trading, Strategies
- **Responsive layout**: dual-column (≥120 cols) / single-column (<120 cols)
- **Rosé Pine theme**: consistent semantic colors (green=positive, red=negative, yellow=warning)
- **gRPC client**: connects to rara-server for real-time data

## Data Flow

### Research → Paper Trading

```
LLM generates hypothesis
    → LLM codes Rust strategy
    → StrategyCompiler builds WASM (wasm32-wasip1)
    → WasmExecutor loads + backtests via barter-rs
    → If accepted: StrategyPromoter saves to ~/.rara-trading/strategies/promoted/
    → Paper trading discovers and loads promoted WASM strategies
    → WasmExecutor.on_candles() generates Signals
    → Guard pipeline validates → PaperBroker executes
```

### Feedback Loop

```
Paper trading publishes order.filled events to EventBus
    → Feedback aggregates metrics over evaluation window
    → Promote: strategy graduates to live trading
    → Hold: continue paper trading
    → Demote: remove from paper trading
    → Retrain: publish feedback to Research for new hypothesis generation
```

### Strategy Registry (External)

```
rara-strategies repo publishes WASM via GitHub Releases
    → rara-trading strategy fetch <name>
    → Download .wasm artifact from GitHub API
    → WasmExecutor loads and extracts StrategyMeta
    → Validate API_VERSION compatibility
    → Save to ~/.rara-trading/strategies/promoted/
    → Available for paper/live trading
```

## Key Directories

| Path | Purpose |
|------|---------|
| `~/.rara-trading/config.toml` | Application configuration |
| `~/.rara-trading/strategies/generated/` | Research-generated strategy artifacts |
| `~/.rara-trading/strategies/promoted/` | Accepted strategies ready for trading |
| `strategies/template/` | WASM compilation scaffold (Cargo.toml + lib.rs template) |

## Design Decisions

### Why WASM?

- **Sandboxing** — strategies run in wasmtime with fuel limits, preventing infinite loops or excessive resource usage
- **Portability** — compiled once, runs on any platform with wasmtime
- **Safety** — WASM modules cannot access the filesystem, network, or host memory directly
- **Hot-loading** — new strategies can be loaded without restarting the system

### Why gRPC C/S?

- **Remote monitoring** — TUI can connect from a different machine
- **Multiple clients** — future web dashboard or mobile app can connect simultaneously
- **Decoupled deployment** — server and client can be updated independently

### Why sled for Event Bus?

- **Embedded** — no external dependency, runs in-process
- **Persistent** — events survive process restarts
- **Fast** — B-tree based, handles high-throughput event publishing
- **Simple** — single file storage, no configuration needed
