# rara-trading

A self-iterating closed-loop trading agent system built in Rust. The system autonomously generates strategy hypotheses, backtests them, executes trades, monitors market sentiment, and feeds performance data back into the research loop for continuous improvement.

## Architecture

### Closed Loop

```mermaid
graph LR
    subgraph Research["Research Engine"]
        H[Hypothesis Generator<br/><i>CLI Agent</i>]
        C[Strategy Coder<br/><i>CLI Agent</i>]
        B[Backtester<br/><i>barter-rs</i>]
        T[Trace DAG<br/><i>sled</i>]
    end

    subgraph Trading["Trading Engine"]
        G[Guard Pipeline<br/><i>risk controls</i>]
        X[CcxtBroker<br/><i>Binance/OKX/Bybit</i>]
        P[PaperBroker<br/><i>simulation</i>]
    end

    subgraph Sentinel["Sentinel Engine"]
        R[RSS / Webhook<br/><i>data sources</i>]
        A[Signal Analyzer<br/><i>CLI Agent</i>]
    end

    subgraph Feedback["Feedback Bridge"]
        M[Metrics Aggregator]
        E[Strategy Evaluator]
    end

    EB((Event Bus<br/><i>sled</i>))

    H -->|generate| C
    C -->|code| B
    B -->|results| T
    T -->|best experiment| H

    Research -->|strategy.candidate| EB
    EB -->|commit| G
    G -->|approved| X & P
    X & P -->|order.filled| EB

    Sentinel -->|signal.detected| EB
    A -->|block trading| G

    EB -->|trading events| M
    M --> E
    E -->|promote / demote| EB
    EB -->|retrain| Research
```

### Strategy Lifecycle

```mermaid
stateDiagram-v2
    [*] --> Candidate: research accepts
    Candidate --> Backtesting: backtest triggered
    Backtesting --> PaperTrading: sharpe > 1.0<br/>drawdown < 15%
    Backtesting --> Retired: poor metrics
    PaperTrading --> Live: sustained performance
    PaperTrading --> Retired: degradation detected
    Live --> PaperTrading: drawdown spike
    Live --> Retired: feedback demote
    Retired --> [*]
```

### Research Loop Detail

```mermaid
flowchart TB
    ctx[Market Context] --> gen[Hypothesis Generator]
    gen --> hyp[New Hypothesis]
    hyp --> code[Strategy Coder]
    code --> strat[Strategy Code]
    strat --> bt[Barter Backtester]
    bt --> eval{Sharpe > 1.0?<br/>Drawdown < 15%?}
    eval -->|Yes| accept[Save to Trace DAG<br/>Publish candidate event]
    eval -->|No| reject[Save feedback<br/>Record failure reason]
    accept --> ctx
    reject --> ctx
    trace[(Trace DAG<br/>sled)] -.->|best experiment<br/>ancestor chain| gen
    accept -.-> trace
    reject -.-> trace
```

### Trading Execution Flow

```mermaid
sequenceDiagram
    participant S as Strategy
    participant E as Trading Engine
    participant G as Guard Pipeline
    participant B as Broker
    participant EB as Event Bus

    S->>E: TradingCommit (staged actions)
    E->>EB: order.submitted event
    E->>G: validate actions
    alt Guards pass
        G->>E: approved
        E->>B: push orders
        B->>E: OrderResult (filled/rejected)
        E->>EB: order.filled event
    else Guard blocked
        G->>E: rejected (reason)
        E->>EB: risk.triggered event
    end
    E->>B: sync_orders + positions
```

### Components

| Module | Description | Key Types |
|--------|-------------|-----------|
| **Research Engine** | RD-Agent style hypothesis → code → backtest → evaluate loop | `ResearchLoop`, `HypothesisGenerator`, `StrategyCoder`, `BarterBacktester`, `Trace` (DAG) |
| **Trading Engine** | OpenAlice style stage → commit → guard → push → sync execution | `TradingEngine`, `GuardPipeline`, `CcxtBroker`, `PaperBroker` |
| **Sentinel Engine** | Market surveillance for black swan detection | `SentinelEngine`, `SignalAnalyzer`, `RssDataSource`, `WebhookDataSource` |
| **Feedback Bridge** | Performance evaluation and strategy lifecycle management | `FeedbackBridge`, `MetricsAggregator`, `StrategyEvaluator` |
| **Event Bus** | sled-backed persistent event bus with broadcast notifications | `EventBus`, `EventStore` |
| **Domain Models** | Contract types, strategies, orders, signals | `Contract`, `SecType`, `Strategy`, `StagedAction`, `TradingCommit` |

### Supported Markets

- **Crypto Spot** — via ccxt-rust (Binance, OKX, Bybit)
- **Crypto Perpetual/Futures** — leveraged trading with funding rate awareness
- **Stocks** — planned (Alpaca integration)
- **Prediction Markets** — planned (Polymarket)

### Strategy Types

- Directional (trend following, mean reversion)
- Cross-exchange arbitrage
- Pairs trading
- Prediction market arbitrage
- Basis arbitrage (spot vs futures)

## Tech Stack

- **Language**: Rust (edition 2024)
- **Async runtime**: tokio
- **Persistence**: sled (event bus, trace DAG)
- **Backtesting**: barter-rs
- **Exchange connectivity**: ccxt-rust
- **Agent execution**: CLI executor (Claude, Kiro, Gemini, Codex, etc.)
- **Error handling**: snafu
- **Timestamps**: jiff
- **Financial math**: rust_decimal

## Local Development

```bash
# Build and run
cargo run -- --help

# Run tests
cargo test

# Lint
cargo clippy --all-targets --all-features -- -D warnings
```

## Project Status

See [Issue #1](https://github.com/rararulab/rara-trading/issues/1) for architecture design and progress tracking.

## License

MIT
