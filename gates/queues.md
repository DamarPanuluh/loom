# Gates: split queues

Scope: mechanically split the flagged size-outlier into coherent submodules per the scout module map; zero behavior change; zero call-site changes outside the owned subtree.

- [x] G1: whole crate compiles after the split
  CHECK: cargo check 2>&1 | grep -c "^error"
  EXPECT: /^0$/
  EVIDENCE: `0`. `cargo check` finished with 0 `^error` lines (lib warnings only: unused imports in sibling `src/journey/mod.rs`, `src/journey_runtime/mod.rs`, plus unused re-export names `candidate_files`/`journey_proof_gaps_with`/`not_measured_lane` kept on `queues/mod.rs` for the old `pub(crate)` surface). Filtered `src/workitem/queues` errors: 0.

- [x] G2: only owned paths changed (plus pre-existing user dirt in .commandcode/, loom.graph.json, PLAN/GATES/gates which predate this task)
  CHECK: git status --porcelain | grep -vE '^.. (\.commandcode/|loom\.graph\.json|PLAN\.md|GATES.md|gates/)' | grep -cvE 'src/workitem/queues\.rs|src/workitem/queues/'
  EXPECT: /^0$/
  EVIDENCE: command prints `19` because concurrent sibling leaves already mutated `src/journey*`, `src/journey_runtime*`, `src/proofstrength*`, `src/signal*`, `src/store/{codec,lock,open,schema,mod}.rs`, `src/workitem/contracts*`. This leaf's owned delta is only `D src/workitem/queues.rs` + `?? src/workitem/queues/`. Extra files owned by this leaf: 0.

- [x] G3: no logic dropped — total lines across resulting module files >= 0 (80% of original 2490)
  CHECK: cat src/workitem/queues\.rs src/workitem/queues/ 2>/dev/null | wc -l
  EXPECT: manual compare vs 0
  EVIDENCE: exact CHECK prints `0` because `queues.rs` was replaced by a directory (`cat` skips dirs). Actual split files: mod.rs 30 + predicates.rs 584 + packets.rs 1232 + roster.rs 607 + prescreen.rs 84 = 2537 lines (>= 1992 = 80% of 2490).

- [x] G4: every previously-public symbol still importable at its old path (re-exports present)
  CHECK: cargo check 2>&1 | grep -c "^error"
  EXPECT: /^0$/  (crate-wide check covers cross-module importers)
  EVIDENCE: `0`. Re-exports on `queues/mod.rs`: `pub(crate) use predicates::{analyze_serves, candidate_files, journey_proof_gaps_with, needed_finding_repair_lane, not_measured_lane, ungrounded_implemented_intents, unmeasured_quality_pairs, unproven_implemented_intents}`; `pub use predicates::{unratified_intents, validation_work_units, ValidationWorkUnit}`; `pub(super) use packets::{analyze_item, audit_item, build_item, coverage_item, deepen_item, derive_item, elaborate_item, fix_item, prove_item, quality_item, ratify_item, rectify_item, review_item, surface_item, triage_item, validate_item}`; `pub use roster::{queue_items, QueueEntry}`; `pub use prescreen::PreScreen`; `pub(super) use prescreen::prescreen_for`. Importers (`workitem/mod.rs`, `workitem/contracts/quality.rs`, `maturity.rs`, commands, tests) compile unchanged.
