# rara-trading

A self-iterating closed-loop trading system built in Rust. Inspired by [RD-Agent](https://github.com/microsoft/RD-Agent), the system autonomously proposes strategy hypotheses, validates them through backtesting and paper trading, and promotes proven strategies to live execution.

## System Overview

The system consists of **independent components**, each running its own loop. Components are decoupled and communicate through an **event bus** (sled-backed persistent messaging).

```mermaid
block-beta
    columns 3

    block:Research["Research Engine"]:3
        columns 3
        r1["Propose Hypothesis"] --> r2["Code Strategy"] --> r3["Backtest"]
        r3 --> r1
    end

    space:3

    block:Gate1["Gate: sharpe > 1.0 & drawdown < 15%"]:3
        columns 1
        g1["Only validated strategies pass"]
    end

    space:3

    block:Paper["Paper Trading"]:3
        columns 3
        p1["Execute (simulated)"] --> p2["Track PnL"] --> p3["Evaluate"]
        p3 --> p1
    end

    space:3

    block:Gate2["Gate: sustained win rate & risk metrics"]:3
        columns 1
        g2["Only proven strategies pass"]
    end

    space:3

    block:Live["Live Trading"]:3
        columns 3
        l1["Execute (exchange)"] --> l2["Track PnL"] --> l3["Evaluate"]
        l3 --> l1
    end

    Research --> Gate1 --> Paper --> Gate2 --> Live
    Live --> Research

    style Research fill:#2d4a2d,color:#fff
    style Paper fill:#4a3d2d,color:#fff
    style Live fill:#4a2d2d,color:#fff
    style Gate1 fill:#333,color:#ff0
    style Gate2 fill:#333,color:#ff0
```

### How Each Component Works

**Research Engine** — proposes and validates strategies autonomously

```mermaid
flowchart LR
    subgraph inputs["Inputs"]
        M["Market Data<br/>(price, volume, trends)"]
        F["Past Performance<br/>(what worked, what failed)"]
    end

    subgraph loop["Research Loop"]
        direction TB
        H["Agent proposes hypothesis"] --> C["Agent codes strategy"]
        C --> B["Backtest on historical data<br/>(barter-rs)"]
        B --> E{"Meets threshold?"}
        E -->|No, learn from failure| H
    end

    inputs --> loop
    E -->|"Yes → publish candidate"| OUT["Event Bus"]
```

**Trading Engine** — executes with risk controls

```mermaid
flowchart LR
    IN["Event Bus<br/>(new candidate or signal)"] --> G["Guard Pipeline<br/>- Max position size<br/>- Drawdown limit<br/>- Sentinel gate"]
    G -->|Blocked| R["Reject + log reason"]
    G -->|Approved| B["Broker<br/>PaperBroker / CcxtBroker"]
    B --> T["Track fills + PnL"]
    T --> OUT["Event Bus<br/>(order.filled events)"]
```

**Sentinel** — monitors for black swan events (runs independently)

```mermaid
flowchart LR
    S1["RSS Feeds"] --> A["Agent classifies signal"]
    S2["Webhooks"] --> A
    A --> D{"Severity?"}
    D -->|"Critical"| B["Block all trading"]
    D -->|"Warning"| W["Alert + reduce exposure"]
    D -->|"Info/None"| I["Log only"]
```

**Feedback Bridge** — closes the loop

```mermaid
flowchart LR
    IN["Event Bus<br/>(trading events)"] --> AGG["Aggregate metrics<br/>sharpe, drawdown, win rate"]
    AGG --> EVAL{"Evaluate"}
    EVAL -->|Promote| UP["Paper → Live"]
    EVAL -->|Hold| HOLD["Keep running"]
    EVAL -->|Demote| DOWN["Live → Paper or retire"]
    EVAL -->|Retrain| RE["Event Bus → Research Engine<br/>(trigger new hypothesis)"]
```

## Key Design Principles

1. **Components are decoupled** — each runs independently, communicates via event bus polling
2. **Stage gates with clear thresholds** — strategies must earn their way from research → paper → live
3. **Agent-driven research** — hypotheses come from analyzing both market data AND past trading performance
4. **No mocks** — all components are real implementations (ccxt-rust, barter-rs, RSS feeds)

## Supported Markets

| Market | Broker | Status |
|--------|--------|--------|
| Crypto Spot | ccxt-rust (Binance, OKX, Bybit) | Implemented |
| Crypto Perpetual | ccxt-rust | Implemented |
| Stocks | Alpaca | Planned |
| Prediction Markets | Polymarket | Planned |

## Tech Stack

Rust 2024, tokio, sled, barter-rs, ccxt-rust, snafu, jiff, rust_decimal

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
