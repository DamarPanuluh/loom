---
type: module
title: "hypothesis plane"
tags:
  - workflow
sourceFiles:
  - src/cli.rs
  - src/commands/hypothesis.rs
  - src/commands/next/modes.rs
  - src/commands/sync.rs
  - src/commands/validation.rs
  - src/db/queries/smells.rs
  - src/db/queries/smells/lifecycle.rs
  - src/db/schema.rs
  - src/db/sqlite/edge_writes.rs
  - src/types.rs
symbols:
  - HypothesisCmd
  - detect_hypothesis_accumulation
  - flag_targets
  - fn run_prove
  - loom hypothesis add
  - prepare_mark_result
  - pub const HYPOTHESIS
  - pub enum HypothesisCmd
  - pub fn run
  - pub struct Hypothesis
  - resolve_hypothesis_with_db
  - run_with_sqlite
  - set_targets_status_for_hypothesis
provenance:
  src/cli.rs: 55a1c8e397f8534e
  src/commands/hypothesis.rs: a8cf4a070e0b66b9
  src/commands/next/modes.rs: 3501df180644d7c4
  src/commands/sync.rs: 93ff4e43a7481814
  src/commands/validation.rs: d726cbcb95e66827
  src/db/queries/smells.rs: e70919e9d98f7fae
  src/db/queries/smells/lifecycle.rs: 407a6815b60abe72
  src/db/schema.rs: e561ff6cbb65f572
  src/db/sqlite/edge_writes.rs: a5823037fd573ddb
  src/types.rs: 5e23066c85babdab
---

# hypothesis plane

> The pre-decision plane: improvement hypotheses that any lane can propose, an analyzer proves against current code, and a builder adopts into planned intents. Speculation stays invisible to coverage and completeness until adoption converts it into the existing lifecycle.

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
