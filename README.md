# rara-trading (WIP)

Work in progress.

## Vision

`rara-trading` is being built as a closed-loop multi-agent trading system:

- `ACP` as the control plane (state, events, coordination)
- `RD-Agent` as the research loop (hypothesis -> experiment -> evaluation -> iteration)
- `OpenAlice` as the execution loop (paper/live execution with risk controls)
- A feedback bridge from execution performance back into research

Architecture draft: [Issue #1](https://github.com/crrow/rara-trading/issues/1)

## Current Status

- Bootstrapped from Rust CLI template
- Early architecture phase
- Not production-ready

## Local Development

```bash
cargo run -- --help
cargo test
```
