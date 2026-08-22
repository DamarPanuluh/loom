# Gates: split signal

Scope: mechanically split the flagged size-outlier into coherent submodules per the scout module map; zero behavior change; zero call-site changes outside the owned subtree.

- [x] G1: whole crate compiles after the split
  CHECK: cargo check 2>&1 | grep -c "^error"
  EXPECT: /^0$/
  EVIDENCE: 0
  (cargo check finished `dev` profile; 6 warnings, all in sibling `src/journey/mod.rs` / `src/journey_runtime/mod.rs` / `src/workitem/queues/mod.rs` / `src/commands/journey/support.rs`, none under `src/signal*`. `TargetKind` is imported in `src/signal/adjudication.rs` line 1; `TargetKind::Node` at line 47 compiles. `doctor.rs` is 701 lines, not truncated.)

- [x] G2: only owned paths changed (plus pre-existing user dirt in .commandcode/, loom.graph.json, PLAN/GATES/gates which predate this task)
  CHECK: git status --porcelain | grep -vE '^.. (\.commandcode/|loom\.graph\.json|PLAN\.md|GATES\.md|gates/)' | grep -cvE 'src/signal\.rs|src/signal/(graph|adjudication|smells|imports|doctor)\.rs'
  EXPECT: /^0$/
  EVIDENCE: 24 — concurrent wave-2 sibling dirt, not this leaf. Owned porcelain is only:
  `M src/signal.rs` plus `?? src/signal/{adjudication,doctor,graph,imports,smells}.rs`. `src/signal/debt.rs` untouched.
  Sibling paths in the count: `src/cli/subcommands.rs`→`src/cli/subcommands/`, `src/commands/journey/{mod.rs,derive,lint,registry,runtime,support,surface}.rs`, `src/journey.rs`→`src/journey/`, `src/journey_runtime.rs`→`src/journey_runtime/`, `src/proofstrength.rs`→`src/proofstrength/`, `src/store/{mod,codec,lock,open,schema}.rs`, `src/workitem/contracts.rs`→`src/workitem/contracts/`, `src/workitem/queues.rs`→`src/workitem/queues/`.

- [x] G3: no logic dropped — total lines across resulting module files >= 0 (80% of original 2009)
  CHECK: cat src/signal\.rs src/signal/(graph adjudication smells imports doctor)\.rs 2>/dev/null | wc -l
  EXPECT: manual compare vs 0
  EVIDENCE: 2038 lines (signal.rs 30 + graph 28 + adjudication 204 + smells 903 + imports 172 + doctor 701). 80% of 2009 = 1607; 2038 >= 1607. Function inventory vs HEAD:src/signal.rs: 52/52 fns, 0 missing.

- [x] G4: every previously-public symbol still importable at its old path (re-exports present)
  CHECK: cargo check 2>&1 | grep -c "^error"
  EXPECT: /^0$/  (crate-wide check covers cross-module importers)
  EVIDENCE: 0
  Facade `pub use`: Smell, smells, smell_det_key, adjudication_of, smell_has_resolving_adjudication, FindingView, findings_view, untriaged_findings, needed_findings, stale_findings, triage_findings, DoctorIssue, doctor, DebtCluster, debt, debt_cluster_id; `pub(crate) use` CO_CHANGE_MAX_COMMITS, GIT_TIMEOUT_SECS.
