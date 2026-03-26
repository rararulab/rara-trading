# Getting Started

This guide walks you through setting up rara-trading from scratch and running your first research loop.

## Prerequisites

| Tool | Why | Install |
|------|-----|---------|
| **Rust (edition 2024)** | Build the project | [rustup.rs](https://rustup.rs/) — requires Rust 1.85+ or nightly |
| **Docker** | Run TimescaleDB | [docs.docker.com](https://docs.docker.com/get-docker/) |
| **wasm32-wasip1 target** | Compile WASM strategies | `rustup target add wasm32-wasip1` |

## Quick Start

```bash
# Clone and enter the repo
git clone https://github.com/rararulab/rara-trading
cd rara-trading

# Start TimescaleDB (runs on localhost:5432)
docker compose up -d

# Install the WASM target for strategy compilation
rustup target add wasm32-wasip1

# Build the project
cargo build

# Verify the CLI works
cargo run -- --help
```

## Database

The `docker-compose.yml` starts a [TimescaleDB](https://www.timescale.com/) instance (PostgreSQL 16 + time-series extensions):

| Setting | Value |
|---------|-------|
| Host | `localhost` |
| Port | `5432` |
| User | `rara` |
| Password | `rara` |
| Database | `rara_trading` |

Data is persisted in a Docker named volume (`tsdb_data`), so it survives container restarts. Database migrations run automatically on first CLI use.

### Checking database health

```bash
# Container status
docker compose ps

# Direct connection test
psql postgresql://rara:rara@localhost:5432/rara_trading -c "SELECT 1"
```

## Configuration

Generate a default config file:

```bash
cargo run -- config init
```

The default database URL (`postgresql://rara:rara@localhost:5432/rara_trading`) matches the docker-compose setup — no changes needed for local development.

### LLM backend

The research engine requires an LLM backend. Set it in your config:

```bash
cargo run -- config set agent.backend <backend-name>
```

Use `cargo run -- config list` to see all available settings.

## First Run Workflow

### 1. Fetch historical data

```bash
# Fetch BTC/USDT 1-minute candles from Binance
cargo run -- data fetch --source binance --symbol BTCUSDT --start 2024-01-01 --end 2024-03-01

# Or fetch daily candles from Yahoo Finance
cargo run -- data fetch --source yahoo --symbol SPY --start 2024-01-01 --end 2024-03-01
```

Already-fetched days are skipped automatically — safe to re-run for incremental updates.

### 2. Check data coverage

```bash
cargo run -- data info
```

Returns JSON with all stored instruments, their date ranges, and candle counts.

### 3. Run a research loop

```bash
cargo run -- research run --iterations 5 --contract BTC-USDT
```

The research engine will propose strategy hypotheses, compile them to WASM, backtest against your historical data, and promote strategies that pass the quality gate (Sharpe > 1.0, drawdown < 15%).

### 4. View results

```bash
# List all experiments
cargo run -- research list

# Show details for a specific experiment
cargo run -- research show --experiment-id <id>

# List strategies that passed the quality gate
cargo run -- research promoted
```

## Troubleshooting

### Database unreachable

```
Error: failed to connect to database
```

- Verify the container is running: `docker compose ps`
- Check logs: `docker compose logs timescaledb`
- Ensure port 5432 is not used by another process: `lsof -i :5432`

### LLM backend not configured

```
Error: agent backend not configured
```

- Run `cargo run -- config set agent.backend <backend-name>` to configure an LLM backend
- Verify with `cargo run -- config list`

### WASM target missing

```
Error: target 'wasm32-wasip1' not found
```

- Install the target: `rustup target add wasm32-wasip1`
- If using nightly: `rustup target add wasm32-wasip1 --toolchain nightly`

### Rust edition 2024 not supported

```
error: edition 2024 is not yet stable
```

- Update Rust: `rustup update`
- Rust 1.85+ is required for edition 2024 support

## CLI Reference

| Command | Description |
|---------|-------------|
| `data fetch --source <binance\|yahoo> --symbol SYM --start DATE --end DATE` | Fetch historical candles (idempotent) |
| `data info` | Show data coverage per instrument (JSON) |
| `research run [--iterations N] [--contract C]` | Run N research loop iterations |
| `research list [--limit N]` | List experiment history |
| `research show --experiment-id ID` | Show experiment details |
| `research promoted` | List promoted strategies |
| `config init` | Generate default config file |
| `config set\|get\|list` | Manage configuration |
| `agent <prompt> [--backend B]` | Run a prompt through the LLM backend |

All commands output structured JSON to stdout (human-readable logs go to stderr).
