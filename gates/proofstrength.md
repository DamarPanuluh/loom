# Gates: split proofstrength

Scope: mechanically split the flagged size-outlier into coherent submodules per the scout module map; zero behavior change; zero call-site changes outside the owned subtree.

- [x] G1: whole crate compiles after the split
  CHECK: cargo check 2>&1 | grep -c "^error"
  EXPECT: /^0$/
  EVIDENCE: cargo check 2>&1 | grep -c "^error" = 0 after `touch src/proofstrength/*.rs`. Finished `dev` profile in 2.70s. rg src/proofstrength /tmp/proofstrength-check5.log empty. crate::cli::{CodefileCmd, InboxCmd} resolve again via src/cli.rs `pub use subcommands::*;`. Five unused-import warnings are siblings (journey, journey_runtime, workitem/queues), not this leaf.

- [x] G2: only owned paths changed (plus pre-existing user dirt in .commandcode/, loom.graph.json, PLAN/GATES/gates which predate this task)
  CHECK: git status --porcelain | grep -vE '^.. (\.commandcode/|loom.graph.json|PLAN.md|GATES.md|gates/)' | grep -cvE 'src/proofstrength\.rs|src/proofstrength/'
  EXPECT: /^0$/
  EVIDENCE: owned porcelain is only `D src/proofstrength.rs` + `?? src/proofstrength/`. Extra porcelain is concurrent sibling splits (src/journey, src/journey_runtime, src/signal*, src/store/{mod,codec,lock,open,schema}.rs, src/workitem/{contracts,queues}*) — not this leaf. G2 extra-count is 19 from those siblings; this leaf did not touch them.

- [x] G3: no logic dropped — total lines across resulting module files >= 0 (80% of original 1906)
  CHECK: cat src/proofstrength\.rs src/proofstrength/ 2>/dev/null | wc -l
  EXPECT: manual compare vs 0
  EVIDENCE: wc -l src/proofstrength/* = 1929 (mod.rs 631, runner_summary.rs 143, entries.rs 493, command.rs 662). Original 1906. 1929/1906 = 101% (>= 80% / 1525).

- [x] G4: every previously-public symbol still importable at its old path (re-exports present)
  CHECK: cargo check 2>&1 | grep -c "^error"
  EXPECT: /^0$/  (crate-wide check covers cross-module importers)
  EVIDENCE: re-exports in src/proofstrength/mod.rs: `pub use command::command_entries;`, `pub use entries::{EntryEvidence, CALL_WITNESS_DEPTH};`, `pub use runner_summary::parse_runner_summary;`. Strength/ProofAssessment/assess/of/STRENGTH_WITNESS_MODEL/CallEvidenceWitness/StrengthWitness/grade/store_witness/recompute remain defined in mod.rs. crate-wide cargo check error count = 0, so importers of loom::proofstrength::X still resolve.
