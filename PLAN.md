# PLAN: Resolve loom debt feed honestly

## Goal
Every cluster in `loom debt` (12 as of 2026-08-22) gets a real judgment and a real action:
split tangled files into coherent modules, or record an evidence-backed cohesion verdict.
Nothing silently deferred; the feed is re-run at the end to prove what changed.

## Decision procedure (per flagged file)
1. Scout (read-only) inventories top-level items, groups them by responsibility,
   measures intra-file coupling between groups.
2. Driver decides: **split** if ≥2 responsibilities with real coupling cost;
   **justify** if one coherent concern despite size.
3. Split leaves execute mechanically: move code, keep public paths stable
   (`pub use` re-exports where callers exist), no behavior change.
4. Record in loom AFTER acting (verdicts stale on content-hash change):
   `loom debt promote <cluster> --evidence ...` then
   `loom finding verdict <id> resolved|justified --reason ... --evidence file:line`.

## Contract (shared surfaces)
- File ownership: each leaf owns exactly ONE flagged path plus NEW sibling module
  files it creates. No leaf edits another flagged file or lib.rs/main.rs unless its
  own split requires registering a submodule inside its OWN file.
- Public API: zero behavior change. Existing `use loom::x::Y` paths must keep working
  (prefer `mod` + re-export inside the same file over moving types across modules).
- Verification: leaf runs `cargo check`; driver runs full `cargo test` at branch gate
  after merging waves (parallel cargo invocations contend on the target lock).
- Recording: only the driver runs `loom debt promote` / `loom finding verdict`,
  after the leaf's changes land, so hashes are final.

## Tree
- L0: resolve all 12 loom debt clusters honestly
  - L1: size outliers (11 leaves, one per file)
    - L2 leaves: journey.rs, journey_runtime.rs, commands/journey/mod.rs,
      queues.rs, store/mod.rs, store/facts.rs, signal.rs, proofstrength.rs,
      workitem/contracts.rs, scan.rs, cli/subcommands.rs
    - Branch gate: cargo test green + all 11 clusters promoted & settled in loom
  - L1: co_change pair cli.rs+commands.rs (single leaf: judge coupling)
    - Branch gate: judgment recorded
  - Root gates: GATES.md

## Status log
- 2026-08-22: baseline captured (git a6e393e, tree dirty with pre-existing user edits — untouched). Baseline test job started.
- 2026-08-22: baseline 1288 passed / 0 failed. 12 scouts done: 9 splits, 3 justifies (facts.rs ring23 chokepoint; scan.rs prod 821<fence, overrun is colocated tests; cli+commands co-change = parse/dispatch contract). 3 justify verdicts promoted+settled in loom (pfcce0f4, pbc20903, pdf075be). Wave 1 dispatched: LeafJourney, LeafJRuntime, LeafStoreMod.
- 2026-08-22: wave 1 landed + driver-verified (cargo check 0 errors; ownership clean; loc conserved: journey 4587/4452, jruntime 4146/4012, store 2305/2168). Wave 2 dispatched: LeafSignal, LeafProofstrength, LeafContracts, LeafQueues.
- 2026-08-22: wave 2 landed + driver-verified (cargo check 0 errors; loc conserved: signal 2038/2009, proofstrength 1929/1906, contracts 1683/1632, queues 2537/2490). LeafSignal fixed its own E0433 TargetKind mid-wave. Wave 3 next: cmdjourney, clisub.
- 2026-08-22: completeness split landed + registered; ring23/ring37 path constants updated to track moved code; cargo test passed=1288 failed=0 (= baseline). Doctor 15→2 (remaining 2 cascade from pre-existing compass-projection failure). Debt feed 12→5, all 5 carry settled verdicts. gate-check: ALL MET 47/47.
