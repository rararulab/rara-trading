# Rust Code Style

## Error Handling

- Use `snafu` exclusively — never `thiserror` or manual `impl Error`
- Every error enum: `#[derive(Debug, Snafu)]` + `#[snafu(visibility(pub))]`
- Name: `{Module}Error`, variants use `#[snafu(display("..."))]`
- Propagate with `.context(XxxSnafu)?` or `.whatever_context("msg")?`
- Define `pub type Result<T> = std::result::Result<T, ModuleError>` per module

## Struct Construction — Use `bon::Builder`

Structs with 3+ fields MUST use `#[derive(bon::Builder)]` — do NOT write manual `fn new()` constructors.

## Struct Fields — Prefer `pub`

Data/value structs SHOULD have `pub` fields. Do NOT write manual getter methods that simply return
`&self.field` or `self.field`. Only write methods that contain real logic (formatting, parsing, computation).

Correct:
- `pub fn id(&self) -> String { format!("{}-{}", self.exchange, self.symbol) }` — computed value
- `pub fn topic(&self) -> &str { self.event_type.split('.').next()... }` — parsing logic
- `pub fn should_block_trading(&self) -> bool { self.severity == Severity::Critical }` — business rule

Wrong:
- `pub fn name(&self) -> &str { &self.name }` — use `pub name: String` instead

## Enum Derives — Use `strum`

All enums that may need string conversion MUST derive `strum::Display` and `strum::EnumString`.
Do NOT write manual `match` blocks for string ↔ enum conversion.

## Async

- Use `tokio` runtime
- `#[async_trait]` + `Send + Sync` bound on async trait definitions

## Functional Style

- **Iterator chains** over `for` loops with manual accumulation
- **Early returns with `?`** over nested `if let` / `match`
- **Combinators on Option/Result** — `.map()`, `.and_then()`, `.unwrap_or_else()`
- **`match` for complex branching** — use when 3+ arms or destructuring needed
- **Immutable by default** — only use `mut` when genuinely needed
- Use `.expect("context")` over `unwrap()` in non-test code

## Code Organization

- Split logic into sub-files; `mod.rs` only for re-exports + module docs
- Imports grouped: `std` → external crates → internal (`crate::` / `super::`)
- No wildcard imports (`use foo::*`)
- All `pub` items must have `///` doc comments in English

## Tests — No Trivial Roundtrips

Do NOT write trivial tests that only exercise derive macros or obvious wiring:
- builder roundtrip tests
- getter roundtrip tests
- serde roundtrip tests

Tests MUST validate real logic, invariants, or integration behavior.
