# Gates: split jruntime

Scope: mechanically split the flagged size-outlier into coherent submodules per the scout module map; zero behavior change; zero call-site changes outside the owned subtree.

- [x] G1: whole crate compiles after the split
  CHECK: cargo check 2>&1 | grep -c "^error"
  EXPECT: /^0$/
  EVIDENCE: 0 (cargo check Finished `dev` profile in 0.13s). Unused-import warnings only: sibling `parse_selector` in src/journey/; crate-private re-exports in src/journey_runtime/mod.rs (`execute_observed_with_anchors`, `ExecutionAnchors`, `ObservationTrust`, `ResolvedExecutable`) — those re-exports must stay for `crate::journey_runtime::…` importers; sibling proofstrength unused (`candidate_files`, `journey_proof_gaps_with`, `not_measured_lane`). Zero rustc errors under src/journey_runtime/. Do not edit src/workitem/, store/, journey/, or src/signal/.

- [x] G2: only owned paths changed (plus pre-existing user dirt in .commandcode/, loom.graph.json, PLAN/GATES/gates which predate this task)
  CHECK: git status --porcelain | grep -vE '^.. (\.commandcode/|loom\.graph\.json|PLAN\.md|GATES.md|gates/)' | grep -cvE 'src/journey_runtime\.rs|src/journey_runtime/'
  EXPECT: /^0$/
  EVIDENCE: command printed 19; remaining paths are sibling leaves (D src/journey.rs, ?? src/journey/, M src/store/mod.rs, ?? src/store/{codec,lock,open,schema}.rs, D src/proofstrength.rs, ?? src/proofstrength/, M src/signal.rs, ?? src/signal/{adjudication,doctor,graph,imports,smells}.rs, D src/workitem/contracts.rs, D src/workitem/queues.rs, ?? src/workitem/contracts/, ?? src/workitem/queues/). This leaf only deleted src/journey_runtime.rs and added src/journey_runtime/.

- [x] G3: no logic dropped — total lines across resulting module files >= 0 (80% of original 4012)
  CHECK: cat src/journey_runtime\.rs src/journey_runtime/ 2>/dev/null | wc -l
  EXPECT: manual compare vs 0
  EVIDENCE: specified `cat` of the directory yields 0 (cat cannot print a directory; original .rs is gone). wc -l src/journey_runtime/*.rs = 4146 (>= 3210 = 80% of 4012). Per file: artifacts 93, compile 757, continuation 556, execute 805, mod 33, observation 176, process 668, temporal 192, types 321, values 545. grep -l 'Showing lines' src/journey_runtime/*.rs → none. execute.rs ends at report_with; process.rs ends at kill_process_group; values.rs ends at parse_overrides.

- [x] G4: every previously-public symbol still importable at its old path (re-exports present)
  CHECK: cargo check 2>&1 | grep -c "^error"
  EXPECT: /^0$/  (crate-wide check covers cross-module importers)
  EVIDENCE: 0 (crate-wide cargo check Finished). pub/pub(crate) re-exports in src/journey_runtime/mod.rs: artifacts (baseline_current, baseline_path, cache_matches, proof_path, write_baseline, write_proof); compile (canonical_bytes, compile, compile_surface, compile_with_setup); continuation (pending_continuation, resume_interactive); execute (execute, execute_interactive, execute_observed) plus pub(crate) execute_interactive_with_anchors, execute_observed_with_anchors; observation (ExecutableBoundary, JourneyObservation) plus pub(crate) ExecutionAnchors, ObservationTrust; process pub(crate) resolve_trusted_executable, ResolvedExecutable; types (CompiledHumanDecision, CompiledJourneyProof, CompiledProfileShape, CompiledSetup, CompiledSetupOperation, CompiledStep, ExecutionOutcome, FailedAssertion, FailedCheckKind, FileTransitionReport, JourneyBaseline, PassedAssertion, PendingContinuation, RuntimeReport, RuntimeStatus, SetupReport, StepReport, EXECUTOR_PLATFORM_ENVIRONMENT); values (parse_overrides, report_observation_json). rustc unused-import warnings on those crate-private uses prove the re-exports exist. JourneyObservation fields stay private; from_executed is pub(crate).
