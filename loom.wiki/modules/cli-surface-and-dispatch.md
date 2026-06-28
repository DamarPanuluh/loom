---
type: module
title: "CLI surface and dispatch"
tags:
  - cli
sourceFiles:
  - build.rs
  - src/cli.rs
  - src/commands/batch.rs
  - src/commands/codefile.rs
  - src/commands/edge.rs
  - src/commands/export.rs
  - src/commands/ignore.rs
  - src/commands/import.rs
  - src/commands/init.rs
  - src/commands/intent.rs
  - src/commands/mod.rs
  - src/commands/note.rs
  - src/commands/resolve.rs
  - src/commands/rule.rs
  - src/commands/validate.rs
  - src/commands/validation.rs
  - src/commands/whoami.rs
  - src/db/mod.rs
  - src/db/queries/intent.rs
  - src/db/queries/scoring.rs
  - src/db/sqlite/writes.rs
  - src/main.rs
symbols:
  - fn dispatch
  - fn git_build_id
  - fn resolve_codefiles_with_db
  - fn resolve_intent_from_snapshot
  - fn run
  - handle_confirm
  - handle_retire
  - handle_update
  - pub fn align_candidates_from_snapshot_notes
  - pub fn resolve_root
  - pub fn ripple_intent_redefinition
  - pub fn run
  - pub fn run_all
  - resolve_intent_with_db
provenance:
  build.rs: f4c28cfa329bcb8a
  src/cli.rs: 674c22fa337705a0
  src/commands/batch.rs: 6a6ccd926bd72787
  src/commands/codefile.rs: 0ee513864c1cef40
  src/commands/edge.rs: db5833205f278f03
  src/commands/export.rs: 35756c578400e3a2
  src/commands/ignore.rs: 7997dfb337dad444
  src/commands/import.rs: 93f197ec9bd9b116
  src/commands/init.rs: a1dc464a4c6cf3e3
  src/commands/intent.rs: ec7ad101ea150ed2
  src/commands/mod.rs: 43c1e9b8bf460ee3
  src/commands/note.rs: 230f52cc758d8442
  src/commands/resolve.rs: 6e16bf8f5c8ea1d0
  src/commands/rule.rs: 6d024c96ac04f22f
  src/commands/validate.rs: 2a39afe499c31948
  src/commands/validation.rs: d726cbcb95e66827
  src/commands/whoami.rs: 2a7a1af42cbd7f32
  src/db/mod.rs: c0c82ec69ea7831f
  src/db/queries/intent.rs: 754f0308ba488a87
  src/db/queries/scoring.rs: a247bbc1b605ec87
  src/db/sqlite/writes.rs: 2612007f692f01e2
  src/main.rs: 2545218dfc241471
---

# CLI surface and dispatch

> clap-derive command definitions and dispatch to command handlers; bare invocation prints orientation

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















<!-- loom:prose-end -->
