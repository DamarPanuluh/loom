# Gates: resolve loom debt feed honestly (12 clusters)

Scope: judge and act on every `loom debt` cluster — split tangled files or record evidence-backed cohesion verdicts in the loom graph.

- [x] G1: baseline recorded before any edit: clean test run + exact debt list snapshot
  CHECK: cat /tmp/loom-debt-baseline.txt | tail -1
  EXPECT: /12 ranked signal/
  EVIDENCE: /tmp/loom-debt-baseline.txt tail reads "12 ranked signal(s) — advisory, never required"; cargo test baseline (pre-edit, job bg_2): passed=1288 failed=0; git HEAD a6e393e.
- [x] G2: every size-outlier cluster adjudicated in loom graph (promoted finding + settled verdict)
  CHECK: loom finding list 2>&1 | grep -cE "^\\[(justified|resolved)\\] p"
  EXPECT: /^15$/
  EVIDENCE: 15 settled debt findings: resolved=p6efc128 journey, p30df825 journey_runtime, p708035f store/mod, p3a486b3 signal, pef8c6cd proofstrength, p4bb07e2 contracts, p186f1a7 queues, pd8a0211 commands/journey, p22e18b3 cli/subcommands, p285783a completeness (10 splits); justified=pfcce0f4 facts, pbc20903 scan, pdf075be co_change, pcdc5311 batch_auth, p2eb2138 extract pair (5 justifies).
- [x] G3: co_change cluster (cli.rs+commands.rs) judged and recorded
  CHECK: loom finding list 2>&1 | grep -c "pdf075be"
  EXPECT: /[1-9]/
  EVIDENCE: finding pdf075be verdict justified — parse plane (src/cli.rs:37 Command enum) + dispatch plane (src/commands.rs:65 run(), 63 Command:: arms) are one surface contract; combined 1437 loc < fence.
- [x] G4: full test suite green after all changes
  CHECK: cargo test 2>&1 | grep -E "^test result:" | awk '{p+=$4; f+=$6} END {print "passed="p" failed="f}'
  EXPECT: /failed=0/
  EVIDENCE: passed=1288 failed=0 (identical to pre-edit baseline). Two structural tests needed path-tracking updates after approved moves: ring23 migration exemption store/mod.rs→store/schema.rs; ring37 settles-list src/journey.rs→src/journey/compile.rs.
- [x] G5: graph integrity after recording — violations reduced and remaining ones are pre-existing campaign items
  CHECK: loom doctor 2>&1 | tail -1
  EXPECT: /doctor found/
  EVIDENCE: doctor 15 → 2 broken chains. Fixed during task: green-graph surface rebind (dead src/workitem/queues.rs → queues/packets.rs + locator facet via edge set-locator), compile+run pass. Remaining 2 = self-audit + system-purpose, both blocked because their audit steps require a clean doctor; root cause is compass-projection's pre-existing failure (the `loom next --mode fix` item at baseline), not module layout.
- [x] G6: debt feed re-run: every remaining signal carries a settled graph verdict
  CHECK: loom debt 2>&1 | tail -1
  EXPECT: /5 ranked signal/
  EVIDENCE: feed went 12 → 5 signals (fence recalc 1462→1260 surfaced batch_auth + extract pair mid-task; both adjudicated). Remaining 5 all settled: facts/scan/batch_auth justified, both co_change pairs justified. completeness.rs left the feed after its split.
- [x] G7: report audit — every number in final report re-measured at report time
  EVIDENCE: all counts in the final report re-measured 2026-08-22 via loom debt / loom doctor / cargo test / git status immediately before reporting; ledger pasted from gate-check output.
