# rara-trading

A self-iterating closed-loop trading agent system built in Rust.

## How It Works

The system runs one continuous loop: **research proposes strategies, trading executes them, feedback improves them.**

```mermaid
graph TD
    A["1. Propose Hypothesis<br/>Agent analyzes market data + past performance"] --> B["2. Code Strategy<br/>Agent writes trading logic"]
    B --> C["3. Backtest<br/>barter-rs validates on historical data"]
    C --> D{"Pass?<br/>sharpe > 1, drawdown < 15%"}
    D -->|No| A
    D -->|Yes| E["4. Paper Trade<br/>PaperBroker simulates execution"]
    E --> F["5. Evaluate<br/>Feedback Bridge scores performance"]
    F --> G{"Promote?"}
    G -->|Yes| H["6. Live Trade<br/>CcxtBroker executes on exchange"]
    G -->|Hold| E
    G -->|Demote| A
    H --> F

    I(["Sentinel<br/>RSS / Webhook monitoring"]) -.->|black swan detected| E & H
```

The **Sentinel** runs in parallel — monitoring news feeds and on-chain data. If it detects a black swan event, it blocks trading immediately.

## Components

| Module | What it does |
|--------|-------------|
| **Research Engine** | Agent proposes hypotheses, codes strategies, backtests them |
| **Trading Engine** | Executes trades through guard pipeline → broker |
| **Sentinel** | Monitors market signals, blocks trading on black swans |
| **Feedback Bridge** | Evaluates live performance, decides promote/hold/demote |
| **Event Bus** | Persistent message bus connecting all components (sled) |

## Supported Markets

| Market | Broker | Status |
|--------|--------|--------|
| Crypto Spot | ccxt-rust (Binance, OKX, Bybit) | Implemented |
| Crypto Perpetual | ccxt-rust | Implemented |
| Stocks | Alpaca | Planned |
| Prediction Markets | Polymarket | Planned |

## Tech Stack

Rust (2024 edition), tokio, sled, barter-rs, ccxt-rust, snafu, jiff, rust_decimal

## Development

```bash
cargo run -- --help
cargo test
cargo clippy --all-targets --all-features -- -D warnings
```

## Status

See [Issue #1](https://github.com/rararulab/rara-trading/issues/1) for progress.

## License

MIT
