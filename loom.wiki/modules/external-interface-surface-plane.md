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
  src/cli.rs: 71d8a255ece6db6b
  src/commands/interface.rs: ad57947fed162c99
  src/commands/mod.rs: b40e6568e15be571
  src/commands/next.rs: fbbcb35255f2d17f
  src/commands/next/render.rs: 3a894db5f322fe8f
  src/commands/populate.rs: 84469207cdf39230
  src/commands/saga.rs: 7699669bec2018e5
  src/commands/status.rs: e312120d29b48cf6
  src/db/schema.rs: 6c3341dcc983cc82
  src/db/sqlite.rs: 1bb73bc78f225241
  src/db/sqlite/edge_writes.rs: a5823037fd573ddb
  src/types.rs: 6c6ae58e65299ae4
  tests/sqlite_regression.rs: 7a51d36df3717112
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
