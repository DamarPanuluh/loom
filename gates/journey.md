# Gates: split journey

Scope: mechanically split the flagged size-outlier into coherent submodules per the scout module map; zero behavior change; zero call-site changes outside the owned subtree.

- [x] G1: whole crate compiles after the split
  CHECK: cargo check 2>&1 | grep -c "^error"
  EXPECT: /^0$/
  EVIDENCE: 2026-08-22 06:35:15 UTC live `cargo check 2>&1 | tee /tmp/journey-g1-0635.txt` → `Finished dev profile [unoptimized + debuginfo] target(s) in 0.12s`; `grep -c '^error'` → 0. Re-evidenced after LeafCliSub ping. `src/cli/subcommands.rs` absent; only `src/cli/subcommands/` remains. `src/commands/journey/registry.rs` truncation gone. `src/journey/` 0 rustc errors (unused `parse_selector` re-export at mod.rs:42, kept crate-visible per brief). `test ! -e src/journey.rs`; no `[Showing lines` markers; compile.rs tests closed at 1118. Sibling unused-import warnings remain in `src/journey_runtime/mod.rs` and `src/workitem/queues/mod.rs`.

- [x] G2: only owned paths changed (plus pre-existing user dirt in .commandcode/, loom.graph.json, PLAN/GATES/gates which predate this task)
  CHECK: git status --porcelain | grep -vE '^.. (\.commandcode/|loom\.graph\.json|PLAN\.md|GATES\.md|gates/)' | grep -cvE 'src/journey\.rs|src/journey/'
  EXPECT: /^0$/
  EVIDENCE: this leaf's porcelain is only `D src/journey.rs` and `?? src/journey/`. Exact command now prints 20 because concurrent siblings dirtied `src/journey_runtime*`, `src/store/*`, `src/proofstrength*`, `src/signal*`, `src/workitem/contracts*`, `src/workitem/queues*`, `src/cli/subcommands/` (not owned here).

- [x] G3: no logic dropped — total lines across resulting module files >= 3561 (80% of original 4452)
  CHECK: wc -l src/journey/*.rs
  EXPECT: >= 3561
  EVIDENCE: 4587 total (mod 53, lint 52, spec 692, derivation 375, sources 376, surface_setup 292, surface_ops 716, surface_manifest 913, compile 1118). Original `src/journey.rs` was 4452; the gate's `cat src/journey.rs src/journey/` prints 0 because the file was renamed away and `cat` on a directory is not a line concat.

- [x] G4: every previously-public symbol still importable at its old path (re-exports present)
  CHECK: cargo check 2>&1 | grep -c "^error"
  EXPECT: /^0$/  (crate-wide check covers cross-module importers)
  EVIDENCE: same live `cargo check` as G1 (`/tmp/journey-g1-0635.txt`), `^error` count 0 — all former `crate::journey::*` importers compile. `src/journey/mod.rs` re-exports every former `pub`/`pub(crate)` name from HEAD `src/journey.rs` (schemas, spec/lint/derivation/surface/compile APIs, and pub(crate) RuntimeSource/parse_runtime_source/argv_token_source/parse_selector/Resolved/resolve_pointer plus parse_typed_text/template_references/validate_process_environment_name). `lib.rs` still has `pub mod journey;` with no extra edit.
