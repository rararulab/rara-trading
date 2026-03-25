# What NOT To Do

## Code & Architecture
- Do NOT use manual `impl Display` + `impl Error` — use `snafu`
- Do NOT write manual `fn new()` constructors for structs with 3+ fields — use `#[derive(bon::Builder)]`
- Do NOT hardcode config defaults in Rust code — use the config system
- Do NOT write code comments in any language other than English
- Do NOT write getter methods that just return `&self.field` — make the field `pub` instead
- Do NOT write manual string ↔ enum conversion functions — use `strum` derives
- Do NOT write trivial tests (builder roundtrips, getter roundtrips, serde roundtrips) — they test derive macros, not your logic

## Workflow
- Do NOT work directly on `main` — ALL changes require a worktree + PR, no exceptions
- Do NOT merge locally on `main` — all merges go through GitHub PRs
- Do NOT edit files in the main checkout for 'quick fixes'
- Do NOT report PR as complete before CI is green
