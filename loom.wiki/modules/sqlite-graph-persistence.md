---
type: module
title: "SQLite graph persistence"
tags:
  - db
sourceFiles:
  - .claude/skills/run-loom/fuzz_import.sh
  - src/commands/codefile.rs
  - src/commands/explain.rs
  - src/commands/export.rs
  - src/commands/guide.rs
  - src/commands/import.rs
  - src/commands/migrate.rs
  - src/commands/next.rs
  - src/commands/note.rs
  - src/db/mod.rs
  - src/db/queries/meta.rs
  - src/db/queries/mod.rs
  - src/db/queries/relates_to.rs
  - src/db/queries/snapshot.rs
  - src/db/queries/symbol_match.rs
  - src/db/schema.rs
  - src/db/sqlite.rs
  - src/db/sqlite/writes.rs
  - src/repo.rs
  - src/types.rs
  - src/vec_utils.rs
symbols:
  - contains_identifier_word
  - fn collect_svelte_symbols
  - fn detect_language
  - fn note_surfaces
  - fn parse_json_array
  - fn port
  - fn resolve_dart_spec
  - fn resolve_go_spec
  - fn resolve_kotlin_spec
  - fn resolve_swift_spec
  - pub fn insert_note
  - pub fn run
  - pub fn run_with_db
  - pub struct QuerySnapshot
  - push_unique_nonempty
  - read_repository
  - run_add_with_sqlite
provenance:
  .claude/skills/run-loom/fuzz_import.sh: 7d98d51ec7549ad8
  src/commands/codefile.rs: d887ca066e4fa091
  src/commands/explain.rs: f4b96fd76b56627e
  src/commands/export.rs: 35756c578400e3a2
  src/commands/guide.rs: 0e285dcdf59a944c
  src/commands/import.rs: 93f197ec9bd9b116
  src/commands/migrate.rs: 904a0c11dc0e329d
  src/commands/next.rs: 0aa0c223261f7268
  src/commands/note.rs: 04649ff93c77aedd
  src/db/mod.rs: c0c82ec69ea7831f
  src/db/queries/meta.rs: 5cd15e776c9e445a
  src/db/queries/mod.rs: 4fe0b3e26d53c011
  src/db/queries/relates_to.rs: 59109a37ebe69b38
  src/db/queries/snapshot.rs: 4bdafb877455648c
  src/db/queries/symbol_match.rs: f2cf13a911c33537
  src/db/schema.rs: e561ff6cbb65f572
  src/db/sqlite.rs: 1bb73bc78f225241
  src/db/sqlite/writes.rs: b7cd81dd5f30f359
  src/repo.rs: fca7873e11343725
  src/types.rs: 5e23066c85babdab
  src/vec_utils.rs: 94f14a1584e50bba
---

# SQLite graph persistence

> embedded SQLite graph store behind typed command/repository APIs; schema vocabulary in Rust, physical tables and constraints in SQLite, derived endpoint edge keys, deterministic JSON import/export, and pure snapshot analysis for graph computations

## Responsibility

_(LLM-authored: what this module does and why it exists.)_

## Key entry points

_(LLM-authored: the main entry symbols and where to start reading.)_

## Key flows

_(LLM-authored: how a request travels through this module.)_

## Common modification points

_(LLM-authored: where to look when changing this module's behavior.)_

## Risk points

_(LLM-authored: gotchas, edge cases, and failure modes.)_


<!-- loom:prose-start -->






## What this module does

The persistence core is an embedded SQLite store behind typed command and
repository APIs. Every command opens the database directly with foreign keys,
busy timeout, and WAL pragmas set in `src/db/sqlite.rs`, mutates under a
transaction (the writes live in `src/db/sqlite/writes.rs`), and renders
through the dual-mode output layer.

The schema vocabulary is declared in Rust in `src/db/schema.rs` — labels,
edges, properties, owners, and version — while the physical tables and
constraints live in SQLite. Edges are endpoint-constrained: keyed by endpoint
ids with derived stable edge ids and SQLite uniqueness/foreign-key
constraints, not stored edge uuids.

## Key entry points

- `src/db/mod.rs` — the module boundary.
- `src/db/sqlite.rs` — `SqliteGraphStore::open`.
- `src/db/queries/snapshot.rs` — `QuerySnapshot`.

## Key flows

A `loom <noun> <verb>` invocation resolves its target graph, dispatches to the
noun handler, calls a typed query in `src/db/queries/`, and renders through
`src/output.rs`. The export/import round-trip in `src/commands/export.rs` and
`src/commands/import.rs` lets the graph travel with the repo.

## Common modification points

- Adding a node/edge type: declare in `src/db/schema.rs`, add struct in
  `src/types.rs`, implement repository methods.
- Adding a query: extend `src/db/queries/`.
- Changing pragmas: `src/db/sqlite.rs`.

## Risk points

- Migration proven by `tests/sqlite_regression.rs`.
- Sync ripple bounded by `structure_version` cache in
  `src/db/queries/snapshot.rs`.
- Import hostility: `src/commands/import.rs` rejects partial graphs.
<!-- loom:prose-end -->
