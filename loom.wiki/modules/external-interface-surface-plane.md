---
type: module
title: "external interface surface plane"
tags:
  - validation
sourceFiles:
  - src/cli.rs
  - src/commands/interface.rs
  - src/commands/mod.rs
  - src/commands/next.rs
  - src/commands/next/render.rs
  - src/commands/populate.rs
  - src/commands/saga.rs
  - src/commands/status.rs
  - src/db/schema.rs
  - src/db/sqlite.rs
  - src/db/sqlite/edge_writes.rs
  - src/types.rs
  - tests/sqlite_regression.rs
symbols:
  - INSPECTABLE_PROPS
  - PopulatePulse
  - build_closeout_queues
  - delete_calls_for_validation
  - enum InterfaceCmd
  - fn add_sqlite
  - fn gaps
  - fn orient
  - fn sqlite_interface_gaps_detect_surface_without_calls
  - interface_gaps_plan
  - label::INTERFACE_SURFACE
  - pub fn run
  - run_with_repo
  - sqlite_populate_backfills_interface_calls_from_existing_sagas
  - sqlite_status_surfaces_populate_gap_lane
  - struct InterfaceSurface
provenance:
  src/cli.rs: ba2c8d60d8a75648
  src/commands/interface.rs: ad57947fed162c99
  src/commands/mod.rs: 260aa6729ebe4bb9
  src/commands/next.rs: 8f0e202a7a707e4e
  src/commands/next/render.rs: b41877d629f75561
  src/commands/populate.rs: 84469207cdf39230
  src/commands/saga.rs: d6c547a8139bcf7e
  src/commands/status.rs: 7cde5db284132dbe
  src/db/schema.rs: 429002a41cd81880
  src/db/sqlite.rs: bbee90eb9b991b53
  src/db/sqlite/edge_writes.rs: 47fc4a616ebdd55b
  src/types.rs: d61e6e7631304228
  tests/sqlite_regression.rs: 9f5edeb870d45f8d
---

# external interface surface plane

> Represent externally callable surfaces as first-class graph nodes so ownership, journey coverage, quality rules, and implementation grounding can address an interface independently of the saga YAML that calls it.

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
