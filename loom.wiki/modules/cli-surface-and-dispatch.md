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
  src/cli.rs: 71d8a255ece6db6b
  src/commands/batch.rs: fdeac7cb8993fabd
  src/commands/codefile.rs: d887ca066e4fa091
  src/commands/edge.rs: b25166be8cf6270d
  src/commands/export.rs: 2b5ac56cf0a0e5d8
  src/commands/ignore.rs: 7997dfb337dad444
  src/commands/import.rs: 93f197ec9bd9b116
  src/commands/init.rs: 4eba98b179867b1d
  src/commands/intent.rs: 259ab327032e17d3
  src/commands/mod.rs: b40e6568e15be571
  src/commands/note.rs: 04649ff93c77aedd
  src/commands/resolve.rs: 6e16bf8f5c8ea1d0
  src/commands/rule.rs: 869b1d2e87b7bf3d
  src/commands/validate.rs: 15caf73c44760a19
  src/commands/validation.rs: f0e50da0531773a4
  src/commands/whoami.rs: 2a7a1af42cbd7f32
  src/db/mod.rs: c0c82ec69ea7831f
  src/db/queries/intent.rs: 754f0308ba488a87
  src/db/queries/scoring.rs: 2bf6bc0283dd1fe9
  src/db/sqlite/writes.rs: 05de5ef9d3e93b50
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
