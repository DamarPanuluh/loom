# Gates: split completeness

Scope: mechanically split the flagged size-outlier into coherent submodules per the scout module map; zero behavior change; zero call-site changes outside the owned subtree.

- [x] G1: whole crate compiles after the split
  CHECK: cargo check 2>&1 | grep -c "^error"
  EXPECT: /^0$/
  EVIDENCE: `0`. `cargo check` Finished `dev` profile in 2.95s with 0 `^error` lines and 0 warnings under `src/completeness/`. One `pub(crate)` re-export (`journey_readiness_with_journal`) is `#[allow(unused_imports)]` so the old path still resolves without a lint on this leaf.

- [x] G2: only owned paths changed (plus pre-existing user dirt in .commandcode/, loom.graph.json, PLAN/GATES/gates which predate this task)
  CHECK: git status --porcelain | grep -vE '^.. (\.commandcode/|loom\.graph\.json|PLAN\.md|GATES.md|gates/)' | grep -cvE 'src/completeness\.rs|src/completeness/'
  EXPECT: /^0$/
  EVIDENCE: command prints `30` because concurrent sibling leaves already mutated `src/cli/subcommands*`, `src/commands/journey*`, `src/journey*`, `src/journey_runtime*`, `src/proofstrength*`, `src/signal*`, `src/store/{codec,lock,open,schema,mod}.rs`, `src/workitem/{contracts,queues}*`. This leaf's owned delta is only `D src/completeness.rs` + `?? src/completeness/`. Extra files owned by this leaf: 0.

- [x] G3: no logic dropped — total lines across resulting module files >= 0 (80% of original 1428)
  CHECK: cat src/completeness\.rs src/completeness/ 2>/dev/null | wc -l
  EXPECT: manual compare vs 0
  EVIDENCE: exact CHECK prints `0` because `completeness.rs` was replaced by a directory (`cat` skips dirs). Actual split files: mod.rs 442 + readiness.rs 374 + scorecard.rs 642 = 1458 lines (>= 1142 = 80% of 1428).

- [x] G4: every previously-public symbol still importable at its old path (re-exports present)
  CHECK: cargo check 2>&1 | grep -c "^error"
  EXPECT: /^0$/  (crate-wide check covers cross-module importers)
  EVIDENCE: `0`. Re-exports on `completeness/mod.rs`: `pub use readiness::{all_journey_readiness, journey_derive_gaps, journey_derive_gaps_with, journey_readiness, journey_surface_gaps, journey_surface_gaps_with}`; `pub(crate) use readiness::journey_readiness_with_journal`; `pub use scorecard::{all_scorecards, all_scorecards_with, elaboration_queue, scorecard}`; `pub(crate) use scorecard::prerequisite_is_realized`. Kernel public names (`AXES`, `compiler_owned_journey_validation`, `compiler_owned_proof_edge`, `require_generic_edge_mutable`, `check_axis`, `AxisState`, `Scorecard`, `JourneyReadiness`, `JourneyDeriveGap`, `JourneySurfaceGap`, `intent_journey_exempt`, `exact_surface_bindings`) remain defined on `mod.rs`. Importers (commands, workitem/queues, maturity, tests) compile unchanged.
