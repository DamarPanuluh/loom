# Gates — arch-optimize findings 1–4

Task: resolve every finding from the 2026-08-22 architecture review.
Mode: solo. Working dir: repo root. Closed 2026-08-23.

Ledger: **10 of 10 gates closed** — 9 met, 1 abandoned with reason (G10).

---

## G1 — Broken grounding edge repointed ✔

Edge `6b764bfc` (intent "Loom ranks the weakest standing claim") targeted
`predicates.rs` with locator `deepen_item`, a symbol that lives in
`packets.rs:1115`. Retargeted in place.

- [x] CHECK: `loom edge show 6b764bfc | grep -c 'cfa4696f'`
- [x] EXPECT: `1`
- [x] EVIDENCE: returns `1`; edge line now reads
      `ba48ee7d… → cfa4696f…` (packets.rs). The mis-route came from note
      `f76ebd1e`, whose stated rationale (`build_candidates` /
      `validation_work_units`) describes a different edge.

## G2 — Tangle findings adjudicated ✔

- [x] CHECK: `loom smells | grep '^\[tangled_file' | grep -E 'predicates.rs|packets.rs'`
- [x] EXPECT: every line marked `·justified`
- [x] EVIDENCE: both lines print `[tangled_file·justified]`. `dc3f07a6` and
      `da2299f0` carry settling verdicts with file:line evidence. Cause in both
      cases was grounding density (2 of 16 predicates, 2 of 22 packet builders
      carry an implements edge), not a real tangle.

## G3 — facts.rs size outlier judged ✔

- [x] CHECK: state of promoted finding `pfcce0f47a188e752`
- [x] EXPECT: `justified`
- [x] EVIDENCE: already `justified` in the graph before this session, with
      reasoning matching the independent read: `tests/ring23_chokepoint.rs:22-29`
      pins all fact/evidence SQL to this one filename, and the exemption at :57
      is a path-suffix match, so even `src/store/facts/mod.rs` would be flagged.
      No new verdict needed; the repo had already judged it.

## G4 — No module below the driver plane reaches up into it ✔

- [x] CHECK: `grep -rn 'use crate::commands' src --include='*.rs' | grep -v '^src/commands' | grep -v '^src/main.rs' | wc -l`
- [x] EXPECT: `0`
- [x] EVIDENCE: returns `0`. `src/proof.rs:9` imported
      `crate::commands::truncate`; the rule now lives in `src/text.rs` as
      `ellipsize`. `src/scan.rs:808 truncate_chars` was the same rule written
      twice and is gone. `prescan.rs` (no marker, feeds canonical identity) and
      `pattern.rs` (byte budget) are genuinely different and were left alone.

## G5 — Ring direction enforced by a test, not a comment ✔

- [x] CHECK: `cargo test --test ring65_ring_direction`
- [x] EXPECT: `test result: ok.` 0 failed
- [x] EVIDENCE: `3 passed; 0 failed`. Adversarially verified: appending
      `crate::commands::open_read` to `src/travel.rs` fails the test with
      "src/travel.rs names `crate::commands::`" — it catches real inversions,
      not just its own fixture. Allowlist carries two entries
      (`candidate_surface_policy.rs`, `proofstrength/command.rs`), both of which
      must parse argv against the real `cli::Cli` grammar; a second test drops
      dead allowlist entries.

## G6 — CLI and MCP arg contracts bound ✔

- [x] CHECK: `cargo test --test ring22_mcp_packets`
- [x] EXPECT: `test result: ok.` 0 failed
- [x] EVIDENCE: `29 passed; 0 failed` (4 new). Adversarially verified: setting
      the clap default to a literal `5` fails with `left: "5" right: "3"`.
      Defaults now come from `callgraph::DEFAULT_IMPACT_DEPTH` and
      `runner::DEFAULT_OBSERVE_TIMEOUT_SECS`. The real divergence found — the
      MCP schema declared `minimum 1 / maximum 10` while the CLI enforced no
      bound at all — is fixed at the seam: `impact_report` (which both surfaces
      call) now enforces `max_impact_depth`, so the bound cannot hold on one
      surface and lapse on the other. Both limits are listed by `loom limits`.

## G7 — Dispatcher holds routing, not policy ✔

- [x] CHECK: `awk 'NR>=65 && NR<=240' src/commands.rs | grep -c 'bail!'`
- [x] EXPECT: `0`
- [x] EVIDENCE: returns `0`. Three policy blocks moved to the handlers that own
      them: `apply_cmd::dispatch` (the `--schema`-replaces-file rule),
      `status_cmd::next_dispatch` (the 35-line `(mode, all, full)` rule), and
      `mcp::transcript_cmd` (the `--json` requirement). Remaining `bail!`s in
      the file are inside helper functions, not the dispatch match.

## G8 — docs/commands.md matches the shipped surface ✔

- [x] CHECK: `for c in impact decide deepen absorb; do grep -qE "loom $c" docs/commands.md || echo MISSING:$c; done`
- [x] EXPECT: no output
- [x] EVIDENCE: no output. All four documented with real flags read from
      `--help`. `loom next --full` was also undocumented and is now described
      with the rule that governs it. The stale "removed/deferred command
      families: impact preview" entry no longer implies `loom impact` is absent.

## G9 — Repository-native checks green ✔

- [x] CHECK: `cargo clippy --all-targets -- -D warnings`
- [x] EXPECT: no warnings/errors
- [x] EVIDENCE: 0 warning/error lines.

- [x] CHECK: `cargo test`
- [x] EXPECT: every suite `ok.`, 0 failed
- [x] EVIDENCE: `TOTAL passed: 1301  failed: 0`.

## G10 — Graph integrity clean, no new open findings ✖ ABANDONED

ABANDON: G10 — `loom doctor` cannot be returned to clean in this session, and
forcing it would require fabricating proof outcomes.

What happened: after the code changes, `loom sync` staled 113 claims. Re-running
the validation suite and all 33 Journey proofs cleared more than it staled
(`needs_reverification` 177 baseline → 290 → **169 now**, below where it
started). But the sweep also exposed and caused two separate problems:

1. **Three Journey proofs were carrying recorded greens over code deleted in
   commit `0ba2cb3`.** `journeys/surfaces/impact.surface.json` asserted a caller
   in `src/proofstrength.rs`, a file that has not existed for three commits; the
   validation still read `passed` because nothing had re-run it. Fixed here
   (repointed to `src/proofstrength/entries.rs`, re-accepted, recompiled — now
   12/12). `debt-promotion` hardcodes cluster ids `c30df825` and `c6efc128` for
   `src/journey_runtime.rs` and `src/journey.rs`, both also split in `0ba2cb3`,
   so neither cluster exists and the proof exits 1. Recorded as finding
   `2d2e40b8`; the correct repair is a design decision about whether a surface
   manifest may hardcode a content-addressed id at all.

2. **Running credential-gated proofs in an ordinary session destroyed their
   recorded outcomes.** `release-workflow:proof` needs a one-shot derivation
   authority token and is designed to be unrunnable outside a release cut;
   attempting it moved its exercises edges to `blocked`, which `doctor` treats
   as breaking a proof chain (it tolerates `uninspected`). This was my error —
   the sweep should have been scoped to the journeys the change touched.
   Restoring the prior status by hand would be exactly the fabrication
   `loom audit` exists to catch, so it is left honest and recorded as finding
   `4b36151a`.

Residual state: `doctor` reports 6 `broken_journey_proof_chain` issues, all
rooted in `release-workflow`, `debt-promotion`, `semantic-commit-recommendation`
and the `compass-projection` cycle (failing proof → doctor dirty → integrity
INVALID → compass reroutes to audit → same proof fails). `sync-ripple` and
`dependents` recovered on recompile; `impact` is fixed.

Also open: `src/text.rs` is the one unowned codefile, blocking the `covered`
rung. No authored intent asserts display truncation, and inventing an owner
would fabricate ownership; the coverage lane serves it as real work.

---

## Out of scope (declared, not dropped)

Debt clusters `src/scan.rs` (1603 loc) and `src/batch_auth.rs` (1327 loc) were
named as un-inspected in the review and were not part of "resolve them all".
They remain on the advisory feed.
