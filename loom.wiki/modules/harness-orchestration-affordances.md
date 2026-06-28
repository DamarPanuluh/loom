---
type: module
title: "harness orchestration affordances"
tags:
  - workflow
sourceFiles:
  - src/commands/guide.rs
  - src/commands/next/scoring.rs
  - src/commands/next/slice_filter.rs
  - src/commands/slice.rs
symbols:
  - fn add_dispatch_for_lane
  - fn compute_slices_from_parts
  - fn parallel_safety_for_role
  - fn run_orchestrate_hat
  - struct SlicedRepo
provenance:
  src/commands/guide.rs: 86edf891a92dd567
  src/commands/next/scoring.rs: 8de823d7fa4351fe
  src/commands/next/slice_filter.rs: 5e94c82f0f1426b0
  src/commands/slice.rs: 63f464505a784d48
---

# harness orchestration affordances

> loom exposes scheduling-advice facts (work slices, conflicts, parallel-safety class, strength tier) and an orchestrator topology hat so any external harness can safely fan out subagents; loom informs and advises while the harness executes

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
