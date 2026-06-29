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
  src/cli.rs: a5eb5eb0002de356
  src/commands/interface.rs: ad57947fed162c99
  src/commands/mod.rs: 91cf32d3f7d5791e
  src/commands/next.rs: a62dc0d0aa244efc
  src/commands/next/render.rs: 1e13489c1e318c3c
  src/commands/populate.rs: 84469207cdf39230
  src/commands/saga.rs: d6c547a8139bcf7e
  src/commands/status.rs: 2d71e331f9e98d6a
  src/db/schema.rs: 5c317eba2c3cc095
  src/db/sqlite.rs: 69643931759567f5
  src/db/sqlite/edge_writes.rs: 47fc4a616ebdd55b
  src/types.rs: 8374d2bd55fe6d34
  tests/sqlite_regression.rs: d66c0323448ed627
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
