# Gates: split clisub

Scope: mechanically split the flagged size-outlier into coherent submodules per the scout module map; zero behavior change; zero call-site changes outside the owned subtree.

- [x] G1: whole crate compiles after the split
  CHECK: cargo check 2>&1 | grep -c "^error"
  EXPECT: /^0$/
  EVIDENCE: `0` (2026-08-22; `cargo check` finished `dev` profile, 0 errors; 5 unused-import warnings in sibling modules `src/journey/mod.rs`, `src/journey_runtime/mod.rs`, `src/workitem/queues/mod.rs` — not this leaf)

- [x] G2: only owned paths changed (plus pre-existing user dirt in .commandcode/, loom.graph.json, PLAN/GATES/gates which predate this task)
  CHECK: git status --porcelain | grep -vE '^.. (\.commandcode/|loom\.graph\.json|PLAN\.md|GATES\.md|gates/)' | grep -cvE 'src/cli/subcommands\.rs|src/cli/subcommands/'
  EXPECT: /^0$/
  EVIDENCE: owned tree only: `D src/cli/subcommands.rs` + `?? src/cli/subcommands/`. `src/cli.rs` unmodified. Raw CHECK prints `21` because concurrent wave-3 siblings (journey, journey_runtime, proofstrength, signal, store, workitem/contracts, workitem/queues) are also converting; none of those paths were edited by this leaf.

- [x] G3: no logic dropped — total lines across resulting module files >= 0 (80% of original 1611)
  CHECK: cat src/cli/subcommands\.rs src/cli/subcommands/ 2>/dev/null | wc -l
  EXPECT: manual compare vs 0
  EVIDENCE: original 1612 loc. Resulting `wc -l src/cli/subcommands/*.rs` = 1651 (102% of original; above 80%/1289). The written `cat …/subcommands/` check prints `0` because `cat` cannot read a directory after the `.rs`→dir conversion; line inventory is the `wc -l` of the 13 new files. All 40 original `pub enum` types plus `impl From<ScanFormatArg>` and `impl ClaimRoleArg::owner_role` moved intact.

- [x] G4: every previously-public symbol still importable at its old path (re-exports present)
  CHECK: cargo check 2>&1 | grep -c "^error"
  EXPECT: /^0$/  (crate-wide check covers cross-module importers)
  EVIDENCE: `0`. `src/cli.rs` still `mod subcommands; pub use subcommands::*;` (untouched). `src/cli/subcommands/mod.rs` has `pub use {pattern,intent,codefile,release,edge,capture,diagnostics,proof,domain,proposal,journey,ops}::*;` so `crate::cli::{IntentCmd, …}` paths keep compiling. Crate-wide `cargo check` is the importer proof.
