# Gates: split contracts

Scope: mechanically split the flagged size-outlier into coherent submodules per the scout module map; zero behavior change; zero call-site changes outside the owned subtree.

- [x] G1: whole crate compiles after the split
  CHECK: cargo check 2>&1 | grep -c "^error"
  EXPECT: /^0$/
  EVIDENCE: 0. Latest `cargo check` finished `dev` profile for loom v0.35.4 in 0.27s with 0 `^error` lines. 5 unused-import warnings only, all outside this leaf (`src/journey/mod.rs`, `src/journey_runtime/mod.rs`, `src/workitem/queues/mod.rs`). Transient E0603 in `contracts/quality.rs` was LeafQueues: `prescreen_for` was `pub(super)` in `queues/prescreen.rs` so `pub(super) use` from `queues/mod.rs` was private to workitem. After they widened it to `pub(in super::super)`, quality.rs compiles. No remaining error path under `src/workitem/contracts/`.

- [x] G2: only owned paths changed (plus pre-existing user dirt in .commandcode/, loom.graph.json, PLAN/GATES/gates which predate this task)
  CHECK: git status --porcelain | grep -vE '^.. (\.commandcode/|loom\.graph\.json|PLAN\.md|GATES\.md|gates/)' | grep -cvE 'src/workitem/contracts\.rs|src/workitem/contracts/'
  EXPECT: /^0$/
  EVIDENCE: this leaf's porcelain is only `D src/workitem/contracts.rs` and `?? src/workitem/contracts/`. Filter count is 19 because concurrent wave-2 sibling dirt is live: `src/journey.rs`/`src/journey/`, `src/journey_runtime.rs`/`src/journey_runtime/`, `src/proofstrength.rs`/`src/proofstrength/`, `src/signal.rs`/`src/signal/*`, `src/store/{mod,codec,lock,open,schema}.rs`, `src/workitem/queues.rs`/`src/workitem/queues/`. This leaf did not edit those paths.

- [x] G3: no logic dropped — total lines across resulting module files >= 0 (80% of original 1632)
  CHECK: cat src/workitem/contracts\.rs src/workitem/contracts/ 2>/dev/null | wc -l
  EXPECT: manual compare vs 0
  EVIDENCE: `wc -l src/workitem/contracts/*.rs` = 1683 (mod 70, journey 184, build 185, inspect 226, repair 227, quality 140, validate 421, triage 230). Original was 1632; 1683/1632 ≈ 103% (>= 80% / 1306). All 29 factories plus `verdict_write_back` and both consts are present.

- [x] G4: every previously-public symbol still importable at its old path (re-exports present)
  CHECK: cargo check 2>&1 | grep -c "^error"
  EXPECT: /^0$/  (crate-wide check covers cross-module importers)
  EVIDENCE: 0. `src/workitem/queues/` still imports `super::contracts::{…}` and `src/workitem/mod.rs` still uses `super::contracts::unproven_contract`; crate compiles. `contracts/mod.rs` has `pub(super) use` for every factory. Child fns are `pub(in super::super)` so the re-export is visible to `workitem` (plain `pub(super)` on the child is only visible inside `contracts/` and rustc E0364/E0603's the re-export). `verdict_write_back` is `pub(super)` on `mod.rs` per the brief.
