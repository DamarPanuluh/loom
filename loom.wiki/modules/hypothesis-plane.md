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
  - fn fmt_hypothesis
  - fn hypothesis_not_found
  - fn run_list_with_db
  - fn run_prove
  - fn run_show_with_db
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
  src/cli.rs: 3a8abff5585bdd3b
  src/commands/hypothesis.rs: a8cf4a070e0b66b9
  src/commands/next/modes.rs: 3501df180644d7c4
  src/commands/sync.rs: f88a57b61311bbf2
  src/commands/validation.rs: d726cbcb95e66827
  src/db/queries/smells.rs: 9c35f78698a2c3a4
  src/db/queries/smells/lifecycle.rs: 407a6815b60abe72
  src/db/schema.rs: 429002a41cd81880
  src/db/sqlite/edge_writes.rs: 47fc4a616ebdd55b
  src/types.rs: d61e6e7631304228
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
