# loom

You are working on **loom** — a CLI that builds a living intent graph of a
codebase so an LLM agent can understand, build, and clean it up. loom's first
and own dogfood target is *this repo*: the committed `loom.graph.json` is the
graph of loom's own code, and `.loom/graph.sqlite` is the live store.

## Use loom to navigate loom

Don't keep this repo's model in your head — **loom is the model.** It is
self-documenting and expects you to drive it:

- `loom status` — where am I? (compass phase + 360° coverage vector)
- `loom guide` — the driving protocol: planes, lifecycle, playbook, ripple,
  role lanes, done condition
- `loom schema` — the data model: node/edge types, fields, owning roles, the
  inspection state machine
- `loom next` — the single highest-priority work item, with full context
- `loom <command> --help` — per-command flags + EXAMPLE block
- `loom help` — the full command surface

Every read command takes `--json` (the driving mode). Pin a session with
`export LOOM_GRAPH=<path>` so every loom call hits that graph no matter what
`cd` does. After *any* graph change, run `loom export` before committing so the
committed `loom.graph.json` travels with the repo (`loom export --check`
verifies; `loom status` warns on drift).

## Building loom

Rust, pure-Rust deps, embedded SQLite (`rusqlite` bundled), tree-sitter grammars
on by default. Bar: **zero-warning, rustfmt-clean, tests green.** Pre-commit:

```
cargo fmt && cargo clippy --all-targets && cargo build && cargo test
cargo build --no-default-features && cargo test --no-default-features   # fallback heuristic path stays green
```

Test-only items (constants/imports used solely by `#[cfg(test)]` code) carry
`#[cfg(test)]` so the non-test profile stays warning-free — don't `#[allow]` them.

## Where the rest lives

- **Contributor conventions** (the LLM-driver output contract: anchor-after-mutation,
  human/json parity, bounded output, surface-then-dig) and the design-decision
  rationale + codebase-structure snapshot: [`docs/CONTRIBUTING.md`](docs/CONTRIBUTING.md)
- **Full per-command semantics**: [`docs/COMMANDS.md`](docs/COMMANDS.md)
- **The live codebase map**: the graph itself — `loom find` / `loom coverage`

If the three commands above (`status`, `guide`, `schema`) are not enough to
orient you, that is a loom bug, not a docs problem — fix loom, not this file.
