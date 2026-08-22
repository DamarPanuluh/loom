# Gates: split cmdjourney

Scope: mechanically split the flagged size-outlier into coherent submodules per the scout module map; zero behavior change; zero call-site changes outside the owned subtree.

- [x] G1: whole crate compiles after the split
  CHECK: cargo check 2>&1 | grep -c "^error"
  EXPECT: /^0$/
  EVIDENCE: `0` (2026-08-22). `cargo check` Finished `dev` profile in 0.11s; 5 unused-import warnings in sibling-owned modules (`src/journey/mod.rs`, `src/journey_runtime/mod.rs`, `src/workitem/queues/mod.rs`) — none in `src/commands/journey/`. Tail: `warning: loom (lib) generated 5 warnings` / `Finished dev profile [unoptimized + debuginfo] target(s) in 0.11s`

- [x] G2: only owned paths changed (plus pre-existing user dirt in .commandcode/, loom.graph.json, PLAN/GATES/gates which predate this task)
  CHECK: git status --porcelain | grep -vE '^.. (\.commandcode/|loom\.graph\.json|PLAN\.md|GATES\.md|gates/)' | grep -cvE 'src/commands/journey/'
  EXPECT: /^0$/
  EVIDENCE: this leaf's porcelain is only `src/commands/journey/` (`M mod.rs`; `?? derive.rs lint.rs registry.rs runtime.rs support.rs surface.rs`). Workspace CHECK printed `23` — all other wave-1/2/3 sibling paths (`src/cli/subcommands*`, `src/journey*`, `src/journey_runtime*`, `src/proofstrength*`, `src/signal*`, `src/store/{mod,codec,lock,open,schema}.rs`, `src/workitem/{contracts,queues}*`); this leaf did not edit them.

- [x] G3: no logic dropped — total lines across resulting module files >= 0 (80% of original 2818)
  CHECK: cat src/commands/journey/ 2>/dev/null | wc -l
  EXPECT: manual compare vs 0
  EVIDENCE: `cat src/commands/journey/` on Darwin prints 0 (directory). `wc -l src/commands/journey/*.rs` total **2837** newlines (mod 87 + support 121 + registry 457 + lint 69 + derive 1170 + surface 190 + runtime 743). 80% of 2818 = 2254.4; 2837 >= 2254. All 64 original top-level fn/struct/type names present once.

- [x] G4: every previously-public symbol still importable at its old path (re-exports present)
  CHECK: cargo check 2>&1 | grep -c "^error"
  EXPECT: /^0$/  (crate-wide check covers cross-module importers)
  EVIDENCE: `0`. `pub fn dispatch` remains at `commands::journey::dispatch` (commands.rs). `pub(crate) fn journey_add` re-exported (drive_cmd.rs `super::journey::journey_add`). Other handlers re-exported `pub(crate)` from `mod.rs`.
