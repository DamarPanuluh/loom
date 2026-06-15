# `loom serve` — retired

`loom serve` is no longer part of the runtime architecture. The active graph
store is embedded SQLite at `.loom/graph.sqlite`, and every CLI invocation opens
that store directly. JSON automation and human-mode commands use the same direct
path.

The command remains as a stub so old scripts fail with a clear retirement
message instead of an unknown-command error. Do not build new workflows around a
daemon, socket, lock holder, or process manager.

Current operational model:

- Storage: SQLite WAL via `rusqlite`.
- Portable state: `loom.graph.json` through `loom export` / `loom import`.
- Concurrency: direct SQLite connections; no background loom process.
- Binary promotion: build and dogfood `target/debug/loom`, then install only
  after local tests and export checks pass.
